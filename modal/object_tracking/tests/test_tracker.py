"""How the tracker answers an episode it fails on: one SAM 3.1 refuses every time is answered as
untrackable, and anything else is a service error to ask again."""

import queue
import threading

import numpy as np
import pytest
import torch
from video_service.episode import RejectedVideo

from object_tracking.tracker import TrackerPool, TrackerWorker, TrackingFailed
from object_tracking.video import SampledPicture


@pytest.mark.parametrize(
    ("error", "outcome"),
    [
        (RuntimeError("No points are provided; please add points first"), "rejected"),
        (torch.cuda.OutOfMemoryError("CUDA out of memory"), "failed"),
        (RuntimeError("CUDA error: an illegal memory access was encountered"), "failed"),
    ],
)
def test_only_what_sam_refuses_every_time_is_answered_as_untrackable(error, outcome):
    assert TrackerWorker.outcome(error) == outcome


@pytest.mark.parametrize(("kind", "raised"), [("rejected", RejectedVideo), ("failed", TrackingFailed)])
def test_a_workers_failure_reaches_its_request_as_its_kind(kind, raised):
    jobs, answers = queue.Queue(), queue.Queue()
    pool = TrackerPool(jobs, answers, [])

    def worker() -> None:
        job = jobs.get()
        answers.put((job.number, kind, "RuntimeError: No points are provided"))

    threading.Thread(target=worker, daemon=True).start()
    with pytest.raises(raised):
        pool.track([SampledPicture(0, np.zeros((8, 8, 3), np.uint8))], "bottle")
