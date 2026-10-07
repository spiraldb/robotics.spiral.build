"""One episode's pictures sampled five times a second."""

from dataclasses import dataclass

import numpy as np
from video_service.episode import EpisodeVideo, RejectedVideo

# The tracker sees five pictures a second.
SAMPLE_PERIOD_NS = 200_000_000
# A picture this close to its due time is on time: 30 Hz times truncated to whole nanoseconds put
# every sixth picture a few nanoseconds early.
SAMPLE_TOLERANCE_NS = 2_000_000
# The most pictures one request may sample: ten minutes. Longer episodes are refused rather than
# cut short.
MAX_PICTURES = 3_000


@dataclass(frozen=True)
class SampledPicture:
    """One sampled picture: its presentation time in nanoseconds and its height x width x 3 RGB24
    pixels."""

    time_ns: int
    rgb: np.ndarray


class SampledVideo:
    """An episode's pictures as the tracker sees them."""

    def __init__(self, video: EpisodeVideo):
        self._video = video

    def pictures(self) -> list[SampledPicture]:
        """The first picture and then one every 200 ms, in display order; only the sampled ones are
        converted to RGB."""
        sampled, due = [], None
        for frame in self._video.frames():
            if due is not None and frame.pts < due - SAMPLE_TOLERANCE_NS:
                continue
            if len(sampled) == MAX_PICTURES:
                raise RejectedVideo(f"episodes longer than {MAX_PICTURES} sampled pictures are not tracked")
            sampled.append(SampledPicture(frame.pts, frame.to_ndarray(format="rgb24")))
            due = (frame.pts if due is None else max(due, frame.pts)) + SAMPLE_PERIOD_NS
        if not sampled:
            raise RejectedVideo("the video holds no picture")
        return sampled
