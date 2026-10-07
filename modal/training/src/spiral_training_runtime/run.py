"""A run's job on the GPU: prepare the samples, train ACT, report and checkpoint as it goes, and
resume from its last checkpoint when Modal retries it. See docs/training.md#a-run.
"""

from __future__ import annotations

import io
import logging
import math
import queue
import tempfile
import threading
import time
from contextlib import suppress
from pathlib import Path
from types import TracebackType

import numpy as np
import torch

from .act import TrainingSession
from .report import RunReport
from .request import RunRequest
from .samples import Normalization, TrainingBatch, TrainingSamples
from .stop import StopFlags, StopRequest
from .volume import TrainingVolume

LOG = logging.getLogger(__name__)
# Status and history reach the Volume about this often, and at every checkpoint and phase.
FLUSH_SECONDS = 15.0
PREFETCH_BATCHES = 4
MAX_VALIDATION_EVERY = 250


class TrainingJob:
    def __init__(self, volume: TrainingVolume, stop_flags: StopFlags, request_json: str) -> None:
        self._volume = volume
        self._request = RunRequest.model_validate_json(request_json)
        self._files = volume.run(self._request.key)
        self._stop = StopRequest(stop_flags, self._request.key)
        self._flushed_at = -math.inf

    def run(self) -> None:
        with self._volume.reading():
            status = self._files.read_status()
            resume = self._files.read_resume()
        if status is not None and not status.active:
            # A retried call whose run had already ended.
            return
        state = (
            None
            if resume is None
            else torch.load(io.BytesIO(resume), map_location="cpu", weights_only=True)
        )
        report = (
            RunReport(self._request.run_id) if state is None else RunReport.restore(state["report"])
        )
        try:
            self._train(report, state)
        except Exception as error:
            LOG.exception("run %s failed", self._request.run_id)
            report.enter("failed", str(error) or type(error).__name__)
            report.log(f"Training failed: {error}")
            self._flush(report)

    def _train(self, report: RunReport, state: dict | None) -> None:
        request = self._request
        observation = request.config.observation
        episodes = len(request.episodes)
        report.enter(
            "preparing",
            f"Decoding {episodes} episodes and sampling them at {observation.rate_hz:g} Hz.",
        )
        self._flush(report)
        with tempfile.TemporaryDirectory() as scratch:
            samples = TrainingSamples(request, self._volume, Path(scratch))
            session = TrainingSession(request)
            if state is None:
                image = observation.image
                report.log(
                    f"Prepared {episodes} episodes: {samples.ticks} ticks of "
                    f"{len(observation.cameras)} cameras at {image.width}x{image.height}."
                )
            else:
                session.restore(state["session"])
                report.log(f"Resumed from checkpoint {report.status.current_step}.")
            intervals = sum(len(episode.intervals) for episode in request.episodes)
            report.enter(
                "running",
                f"Training ACT on {session.device_name}: {episodes} episodes, "
                f"{intervals} intervals, {samples.ticks} ticks.",
            )
            self._flush(report)
            self._optimize(session, samples, report)

    def _optimize(
        self, session: TrainingSession, samples: TrainingSamples, report: RunReport
    ) -> None:
        """Steps on from the report's current step until the last step or a stop, then saves a
        final checkpoint when the last one is behind."""
        optimization = self._request.config.optimization
        first_step = report.status.current_step + 1
        validation_every = min(
            MAX_VALIDATION_EVERY, max(1, math.ceil(optimization.checkpoint_every / 4))
        )
        # Seeded by the step this attempt starts at, so attempts from one checkpoint draw alike.
        rng = np.random.default_rng([optimization.seed, first_step])
        validation_rng = np.random.default_rng([optimization.seed, first_step, 1])
        started = time.perf_counter() - report.elapsed_seconds
        stopped = False
        with BatchPrefetcher(samples, optimization.batch_size, rng) as batches:
            for step in range(first_step, optimization.steps + 1):
                if self._stop.requested():
                    stopped = True
                    break
                step_started = time.perf_counter()
                batch, loader_ms = batches.next()
                result = session.train_step(batch, step)
                validation_loss = None
                if samples.has_validation and step % validation_every == 0:
                    validation = samples.validation_batch(validation_rng, optimization.batch_size)
                    validation_loss = session.validate(validation).loss
                report.record(
                    {
                        "step": step,
                        "train_loss": result.loss,
                        "validation_loss": validation_loss,
                        "learning_rate": session.learning_rate,
                        "elapsed_seconds": time.perf_counter() - started,
                        "event": None if validation_loss is None else "validation",
                        "examples_per_second": batch.size / (time.perf_counter() - step_started),
                        **session.telemetry(),
                        # With batches prefetched, the wait for one is what loading costs a step.
                        "loader_ms": loader_ms,
                        "device_copy_ms": result.device_copy_ms,
                        "model_ms": result.model_ms,
                    }
                )
                if step % optimization.checkpoint_every == 0:
                    self._checkpoint(session, samples.normalization, report)
                elif time.monotonic() - self._flushed_at >= FLUSH_SECONDS:
                    self._flush(report)
        if stopped:
            report.enter("stopping", "Saving a final checkpoint.")
            self._flush(report)
        if report.status.current_step > report.last_checkpoint_step:
            self._checkpoint(session, samples.normalization, report)
        steps = report.status.current_step
        if stopped:
            report.enter("stopped", f"Stopped after {steps} steps.")
        else:
            report.enter("completed", f"Completed {steps} steps.")
        self._flush(report)

    def _checkpoint(
        self, session: TrainingSession, normalization: Normalization, report: RunReport
    ) -> None:
        """Saves the current step's checkpoint and the state a retry resumes from, then reports
        it; the status lists a checkpoint only once its files are complete."""
        step = report.status.current_step
        with self._volume.writing():
            session.save_checkpoint(self._files.checkpoint(step), normalization, step)
            report.add_checkpoint(created_ms=int(time.time() * 1_000))
            resume = io.BytesIO()
            torch.save({"session": session.state(), "report": report.state()}, resume)
            self._files.write_resume(resume.getvalue())
            self._files.write_status(report.status)
            self._files.write_history(report.history())
        self._flushed_at = time.monotonic()

    def _flush(self, report: RunReport) -> None:
        with self._volume.writing():
            self._files.write_status(report.status)
            self._files.write_history(report.history())
        self._flushed_at = time.monotonic()


class BatchPrefetcher:
    """Draws and assembles training batches on a background thread while the model runs."""

    def __init__(self, samples: TrainingSamples, size: int, rng: np.random.Generator) -> None:
        self._batches: queue.Queue[TrainingBatch | Exception] = queue.Queue(PREFETCH_BATCHES)
        self._closed = threading.Event()
        self._thread = threading.Thread(
            target=self._fill, args=(samples, size, rng), name="batch-prefetch", daemon=True
        )
        self._thread.start()

    def __enter__(self) -> BatchPrefetcher:
        return self

    def __exit__(
        self,
        _type: type[BaseException] | None,
        _error: BaseException | None,
        _traceback: TracebackType | None,
    ) -> None:
        self._closed.set()
        while self._thread.is_alive():
            # A full queue blocks the thread's put; taking a batch lets it see it is closed.
            with suppress(queue.Empty):
                self._batches.get_nowait()
            self._thread.join(timeout=0.1)

    def next(self) -> tuple[TrainingBatch, float]:
        """The next batch, and the milliseconds spent waiting for it."""
        started = time.perf_counter()
        batch = self._batches.get()
        if isinstance(batch, Exception):
            raise batch
        return batch, (time.perf_counter() - started) * 1_000.0

    def _fill(self, samples: TrainingSamples, size: int, rng: np.random.Generator) -> None:
        try:
            while not self._closed.is_set():
                self._batches.put(samples.train_batch(rng, size))
        except Exception as error:
            self._batches.put(error)
