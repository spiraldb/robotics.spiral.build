"""SAM 3.1 tracking on one GPU: worker processes, each holding its own predictor, that share the GPU
through NVIDIA MPS so their kernels run side by side.

One SAM 3.1 session launches a great many small kernels from Python, which leaves a single process
CPU-bound with the GPU idle a third of the time; two processes under MPS fill it. Each request waits
for its own episode while the workers take episodes from one queue.
"""

import logging
import multiprocessing
import os
import shutil
import subprocess
import tempfile
import threading
from concurrent.futures import Future
from dataclasses import dataclass
from pathlib import Path

import pyarrow as pa
from PIL import Image
from video_service.episode import RejectedVideo
from video_service.service import RetryLater

from object_tracking.video import SampledPicture

LOG = logging.getLogger(__name__)
# Each worker holds a SAM 3.1 predictor at its released 1008-pixel size, about 25 GB of an H100 at
# work; three overflow its 80 GB on long episodes.
WORKERS = 2
# How long one episode may take before its request gives up: a ten-minute episode's 3,000 pictures
# at the slowest measured 2.7 pictures a second, with room to spare.
EPISODE_TIMEOUT_S = 1_500
OBJECT = pa.struct(
    [
        pa.field("id", pa.uint32()),
        pa.field("score", pa.float32()),
        pa.field("x", pa.float32()),
        pa.field("y", pa.float32()),
        pa.field("width", pa.float32()),
        pa.field("height", pa.float32()),
    ]
)
# One row per sampled picture, in display order: its time, and every object tracked on it with its
# track, score, and box as fractions of the picture from its top left.
OUTPUT_SCHEMA = pa.schema(
    [
        pa.field("timestamp_ns", pa.uint64(), nullable=False),
        pa.field("objects", pa.list_(OBJECT), nullable=False),
    ]
)


class TrackingFailed(RetryLater):
    """The tracker failed on an episode, such as running out of GPU memory: a service error."""


# What SAM 3.1 raises when it propagates objects left with no conditioning frame. It reaches that
# state on some episodes' pictures every time it tracks them, so asking again cannot help.
SAM_DEAD_END = "No points are provided"


@dataclass(frozen=True)
class TrackingJob:
    """One episode for a worker: its sampled pictures as numbered JPEG files in `folder`."""

    number: int
    folder: str
    prompt: str


class TrackerPool:
    """Worker processes taking episodes from one queue; each request waits for its own answer."""

    def __init__(self, jobs, answers, processes: list):
        self._jobs = jobs
        self._answers = answers
        self._processes = processes
        self._pending: dict[int, Future] = {}
        self._lock = threading.Lock()
        self._numbers = iter(range(1 << 62))
        threading.Thread(target=self._deliver, daemon=True).start()

    @classmethod
    def start(cls, checkpoint: Path, size: int = WORKERS) -> "TrackerPool":
        """Start the MPS server, then `size` workers, and return once every one has loaded."""
        for variable, directory in (("CUDA_MPS_PIPE_DIRECTORY", "/tmp/mps-pipe"), ("CUDA_MPS_LOG_DIRECTORY", "/tmp/mps-log")):
            os.makedirs(directory, exist_ok=True)
            # The workers inherit these, so their CUDA contexts join the server.
            os.environ[variable] = directory
        subprocess.run(["nvidia-cuda-mps-control", "-d"], check=True)
        context = multiprocessing.get_context("spawn")
        jobs, answers = context.Queue(), context.Queue()
        processes = [
            context.Process(target=TrackerWorker.serve, args=(str(checkpoint), jobs, answers), daemon=True)
            for _ in range(size)
        ]
        for process in processes:
            process.start()
        for _ in processes:
            _, kind, detail = answers.get(timeout=600)
            if kind != "ready":
                raise TrackingFailed(f"a tracker worker failed to load: {detail}")
        return cls(jobs, answers, processes)

    def track(self, pictures: list[SampledPicture], prompt: str) -> pa.RecordBatch:
        """Every sampled picture's tracked objects, at its time, in display order."""
        folder = tempfile.mkdtemp(prefix="episode-")
        try:
            for index, picture in enumerate(pictures):
                Image.fromarray(picture.rgb).save(f"{folder}/{index}.jpg", quality=95)
            job = TrackingJob(next(self._numbers), folder, prompt)
            answer = Future()
            with self._lock:
                self._pending[job.number] = answer
            self._jobs.put(job)
            objects = answer.result(timeout=EPISODE_TIMEOUT_S)
        finally:
            shutil.rmtree(folder, ignore_errors=True)
        return pa.record_batch(
            [
                pa.array([picture.time_ns for picture in pictures], pa.uint64()),
                pa.array([objects.get(index, []) for index in range(len(pictures))], pa.list_(OBJECT)),
            ],
            schema=OUTPUT_SCHEMA,
        )

    def close(self) -> None:
        for _ in self._processes:
            self._jobs.put(None)
        for process in self._processes:
            process.join(timeout=30)

    def _deliver(self) -> None:
        """Hand each worker's answer to the request waiting for it."""
        while True:
            number, kind, detail = self._answers.get()
            with self._lock:
                answer = self._pending.pop(number, None)
            if answer is None:
                continue
            if kind == "tracked":
                answer.set_result(detail)
            elif kind == "rejected":
                answer.set_exception(RejectedVideo(f"SAM 3.1 cannot track this episode: {detail}"))
            else:
                answer.set_exception(TrackingFailed(detail))


class TrackerWorker:
    """One worker process: a predictor, and the episodes the queue hands it."""

    @staticmethod
    def serve(checkpoint: str, jobs, answers) -> None:
        """Load a predictor, then track episodes until told to stop. Every message is
        `(job number, kind, detail)`; an episode that fails is answered with why, and the worker frees
        what it held and goes on."""
        import torch
        from sam3.model_builder import build_sam3_multiplex_video_predictor

        logging.basicConfig(level=logging.INFO)
        try:
            # FlashAttention 3 gains about 6% here and moves boxes with its 8-bit attention.
            predictor = build_sam3_multiplex_video_predictor(checkpoint_path=checkpoint, use_fa3=False)
        except Exception as error:  # reported to the pool, which refuses to start
            answers.put((None, "failed", f"{type(error).__name__}: {error}"))
            return
        answers.put((None, "ready", None))
        while (job := jobs.get()) is not None:
            try:
                answers.put((job.number, "tracked", TrackerWorker._track(predictor, job)))
            except Exception as error:  # one episode's failure, not the worker's
                LOG.exception("tracking failed")
                answers.put((job.number, TrackerWorker.outcome(error), f"{type(error).__name__}: {error}"))
                torch.cuda.empty_cache()

    @staticmethod
    def outcome(error: Exception) -> str:
        """How an episode's failure is answered: `rejected` when SAM 3.1 refuses its pictures every
        time, so the episode is answered as one it cannot track; else `failed`, a service error to
        ask again, as running out of GPU memory is."""
        if type(error) is RuntimeError and SAM_DEAD_END in str(error):
            return "rejected"
        return "failed"

    @staticmethod
    def _track(predictor, job: TrackingJob) -> dict[int, list[dict]]:
        """Each sampled picture's objects, by picture index, from one SAM 3.1 session over the job's
        pictures. Frames wait in host memory, so an episode's length never decides what fits on the
        GPU."""
        session = predictor.handle_request(
            dict(type="start_session", resource_path=job.folder, offload_video_to_cpu=True)
        )["session_id"]
        try:
            first = predictor.handle_request(
                dict(type="add_prompt", session_id=session, frame_index=0, text=job.prompt)
            )
            objects = {0: TrackerWorker._objects(first["outputs"])}
            for response in predictor.handle_stream_request(dict(type="propagate_in_video", session_id=session)):
                objects[response["frame_index"]] = TrackerWorker._objects(response["outputs"])
            return objects
        finally:
            predictor.handle_request(dict(type="close_session", session_id=session))

    @staticmethod
    def _objects(outputs: dict) -> list[dict]:
        """One picture's tracked objects, each with its box as fractions of the picture."""
        return [
            {"id": int(track), "score": float(score), "x": float(x), "y": float(y), "width": float(width), "height": float(height)}
            for track, score, (x, y, width, height) in zip(
                outputs["out_obj_ids"], outputs["out_probs"], outputs["out_boxes_xywh"], strict=True
            )
        ]
