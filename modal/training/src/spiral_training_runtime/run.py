"""A run's job on the GPU: prepare its model family's samples, train, report and checkpoint as it
goes, and resume from its last checkpoint when Modal retries it. See docs/training.md#a-run.
"""

from __future__ import annotations

import io
import logging
import math
import tempfile
import time
from contextlib import closing
from pathlib import Path

import numpy as np
import torch

from .family import Family, PreparedRun, Session
from .report import RunReport
from .stop import StopFlags, StopRequest
from .volume import TrainingVolume

LOG = logging.getLogger(__name__)
# Status and history reach the Volume about this often, and at every checkpoint and phase.
FLUSH_SECONDS = 15.0
MAX_VALIDATION_EVERY = 250


class TrainingJob:
    """A run's job, for whichever family its Modal function trains."""

    def __init__(self, volume: TrainingVolume, stop_flags: StopFlags, family: Family) -> None:
        self._volume = volume
        self._family = family
        self._request = family.request
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
        report.enter("preparing", self._family.preparing())
        self._flush(report)
        with tempfile.TemporaryDirectory() as scratch:
            prepared = self._family.prepare(Path(scratch))
            if state is None:
                for line in prepared.logs:
                    report.log(line)
            else:
                prepared.session.restore(state["session"])
                report.log(f"Resumed from checkpoint {report.status.current_step}.")
            report.enter("running", prepared.running)
            self._flush(report)
            self._optimize(prepared, report)

    def _optimize(self, prepared: PreparedRun, report: RunReport) -> None:
        """Steps on from the report's current step until the last step or a stop, then saves a
        final checkpoint when the last one is behind."""
        session, samples = prepared.session, prepared.samples
        optimization = self._request.config.optimization
        first_step = report.status.current_step + 1
        validation_every = min(
            MAX_VALIDATION_EVERY, max(1, math.ceil(optimization.checkpoint_every / 4))
        )
        # Seeded by the step this attempt starts at, so attempts from one checkpoint draw alike.
        seed = np.random.SeedSequence([optimization.seed, first_step])
        validation_rng = np.random.default_rng([optimization.seed, first_step, 1])
        started = time.perf_counter() - report.elapsed_seconds
        stopped = False
        with closing(samples.train_batches(seed, optimization.batch_size)) as batches:
            for step in range(first_step, optimization.steps + 1):
                if self._stop.requested():
                    stopped = True
                    break
                step_started = time.perf_counter()
                batch = next(batches)
                # With batches built ahead, the wait for one is what loading costs a step.
                loader_ms = (time.perf_counter() - step_started) * 1_000.0
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
                        "loader_ms": loader_ms,
                        "device_copy_ms": result.device_copy_ms,
                        "model_ms": result.model_ms,
                    }
                )
                if step % optimization.checkpoint_every == 0:
                    self._checkpoint(session, report)
                elif time.monotonic() - self._flushed_at >= FLUSH_SECONDS:
                    self._flush(report)
        if stopped:
            report.enter("stopping", "Saving a final checkpoint.")
            self._flush(report)
        if report.status.current_step > report.last_checkpoint_step:
            self._checkpoint(session, report)
        steps = report.status.current_step
        if stopped:
            report.enter("stopped", f"Stopped after {steps} steps.")
        else:
            report.enter("completed", f"Completed {steps} steps.")
        self._flush(report)

    def _checkpoint(self, session: Session, report: RunReport) -> None:
        """Saves the current step's checkpoint and the state a retry resumes from, then reports
        it; the status lists a checkpoint only once its files are complete."""
        step = report.status.current_step
        with self._volume.writing():
            session.save_checkpoint(self._files.checkpoint(step), step)
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
