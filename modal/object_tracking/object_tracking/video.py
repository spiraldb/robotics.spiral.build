"""One episode's pictures sampled at the rate a request asks for."""

from dataclasses import dataclass

import numpy as np
from video_service.episode import EpisodeVideo, RejectedVideo

# A picture this close to its due time is on time: 30 Hz times truncated to whole nanoseconds put
# every sixth picture a few nanoseconds early.
SAMPLE_TOLERANCE_NS = 2_000_000
# The most pictures one request may sample: ten minutes at five a second. Longer episodes are refused
# rather than cut short.
MAX_PICTURES = 3_000
# The most pictures a second a request may ask for: every picture of a 30 Hz camera.
MAX_RATE = 30.0


@dataclass(frozen=True)
class SampledPicture:
    """One sampled picture: its presentation time in nanoseconds and its height x width x 3 RGB24
    pixels."""

    time_ns: int
    rgb: np.ndarray


class SampleRate:
    """How many pictures a second the tracker sees, as a request names it in its `rate` metadata."""

    def __init__(self, text: str | None):
        try:
            rate = float(text or "")
        except ValueError:
            rate = 0.0
        if not 0.0 < rate <= MAX_RATE:
            raise RejectedVideo(f"name a sample rate above 0 and at most {MAX_RATE:g} pictures a second")
        self._period_ns = round(1e9 / rate)

    @property
    def period_ns(self) -> int:
        """The time from one sampled picture to the next."""
        return self._period_ns


class SampledVideo:
    """An episode's pictures as the tracker sees them."""

    def __init__(self, video: EpisodeVideo, rate: SampleRate):
        self._video = video
        self._rate = rate

    def pictures(self) -> list[SampledPicture]:
        """The first picture and then one every sample period, in display order; only the sampled
        ones are converted to RGB."""
        sampled, due = [], None
        for frame in self._video.frames():
            if due is not None and frame.pts < due - SAMPLE_TOLERANCE_NS:
                continue
            if len(sampled) == MAX_PICTURES:
                raise RejectedVideo(f"episodes longer than {MAX_PICTURES} sampled pictures are not tracked")
            sampled.append(SampledPicture(frame.pts, frame.to_ndarray(format="rgb24")))
            due = (frame.pts if due is None else max(due, frame.pts)) + self._rate.period_ns
        if not sampled:
            raise RejectedVideo("the video holds no picture")
        return sampled
