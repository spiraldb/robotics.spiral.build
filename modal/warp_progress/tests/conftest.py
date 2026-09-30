from fractions import Fraction

import av
import numpy as np
import pytest


class SyntheticEpisode:
    """A small libx264 Annex-B stream with B-frames, so decode order differs from display order."""

    def __init__(self, pictures: int, rate: int):
        encoder = av.CodecContext.create("libx264", "w")
        encoder.width, encoder.height, encoder.pix_fmt = 64, 48, "yuv420p"
        encoder.time_base = Fraction(1, rate)
        packets = []
        for index in range(pictures):
            rgb = np.full((48, 64, 3), index * 5 % 256, np.uint8)
            frame = av.VideoFrame.from_ndarray(rgb, format="rgb24").reformat(format="yuv420p")
            frame.pts = index
            packets += encoder.encode(frame)
        packets += encoder.encode(None)
        self.payloads = [bytes(packet) for packet in packets]
        self.times_ns = [packet.pts * 1_000_000_000 // rate for packet in packets]


@pytest.fixture
def episode() -> SyntheticEpisode:
    return SyntheticEpisode(pictures=40, rate=30)
