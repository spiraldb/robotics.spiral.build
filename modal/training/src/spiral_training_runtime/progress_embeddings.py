"""The progress model's prepare step: each episode's camera decoded and embedded exactly as Score
progress embeds it, and kept on the job's local disk. See docs/training.md.
"""

from __future__ import annotations

from pathlib import Path
from typing import BinaryIO

import numpy as np
from video_service.episode import EpisodeVideo
from video_service.timing import StageTimes
from warp_progress.warp import PictureEncoder, PresentationTimes

from .progress_windows import EmbeddedCamera, EmbeddedInterval
from .request import ProgressRunRequest
from .streams import CameraStream
from .volume import TrainingVolume


class CameraEmbeddings:
    """Every picture of a run's camera, episode after episode, as Score progress embeds it:
    decoded and converted to RGB by warp-progress, squashed to 224x224 and embedded by its DINOv3
    ViT-B/16 (under fp16 autocast on CUDA), as float32 rows in a file on the job's local disk."""

    def __init__(
        self,
        request: ProgressRunRequest,
        volume: TrainingVolume,
        encoder: PictureEncoder,
        directory: Path,
    ) -> None:
        path = directory / "embeddings.f32"
        intervals: list[EmbeddedInterval] = []
        self._skipped = 0
        rows = 0
        with path.open("wb") as sink:
            for index, episode in enumerate(request.episodes):
                try:
                    times_ns = self._embed_episode(volume.stream(episode.camera), encoder, sink)
                except Exception as error:
                    raise ValueError(f"episode {episode.episode_id}: {error}") from error
                # Each kept interval `[start, end)`: the pictures presented within it.
                for start, end in episode.intervals:
                    first, last = (int(row) for row in np.searchsorted(times_ns, [start, end]))
                    if last == first:
                        self._skipped += 1
                    else:
                        intervals.append(EmbeddedInterval(index, rows + first, last - first))
                rows += len(times_ns)
        if not intervals:
            raise ValueError(f"no kept interval holds a picture of {request.config.model.camera}")
        self._camera = EmbeddedCamera(path, intervals, len(request.episodes))

    @property
    def camera(self) -> EmbeddedCamera:
        return self._camera

    @property
    def skipped(self) -> int:
        """Kept intervals that hold no picture, which no window can be drawn from."""
        return self._skipped

    @staticmethod
    def _embed_episode(stream: Path, encoder: PictureEncoder, sink: BinaryIO) -> np.ndarray:
        """Writes every picture's embedding to `sink` and answers the pictures' presentation times;
        refuses video Score progress would refuse for its clock."""
        units = list(CameraStream.read(stream).access_units())
        video = EpisodeVideo([payload for payload, _ in units], [time_ns for _, time_ns in units])
        times_ns: list[int] = []
        for batch_times, embeddings in encoder.encode(video, StageTimes()):
            sink.write(embeddings.float().cpu().numpy().tobytes())
            times_ns.extend(batch_times)
        PresentationTimes.require_30_hz(times_ns)
        return np.array(times_ns, np.int64)
