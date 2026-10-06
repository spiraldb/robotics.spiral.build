"""A camera stream decoded onto the training clock, resized as a rollout resizes live frames."""

from __future__ import annotations

import itertools
from collections.abc import Iterator

import av
import numpy as np

from .streams import CameraStream


class CameraVideo:
    def __init__(self, stream: CameraStream) -> None:
        self._stream = stream

    def sample(self, ticks_ns: np.ndarray, frames: np.ndarray) -> None:
        """Writes into `frames[i]` the picture presented at or before tick `i`.

        Ticks before the first picture take the first. `frames` is `[ticks, height, width, 3]`
        RGB; each picture that some tick shows is resized to it once, with swscale bilinear.
        """
        height, width = frames.shape[1:3]
        order = np.argsort(ticks_ns, kind="stable")
        sorted_ticks = ticks_ns[order]
        written = 0
        held: av.VideoFrame | None = None
        for picture in self._pictures():
            if held is not None:
                if picture.pts < held.pts:
                    raise ValueError("decoded pictures are not in presentation order")
                # The held picture shows at every tick before this one is presented.
                shown_until = int(np.searchsorted(sorted_ticks, picture.pts, side="left"))
                if shown_until > written:
                    frames[order[written:shown_until]] = self._resized(held, width, height)
                    written = shown_until
            held = picture
        if held is None:
            raise ValueError("the camera stream decoded to no pictures")
        if written < len(order):
            frames[order[written:]] = self._resized(held, width, height)

    def _pictures(self) -> Iterator[av.VideoFrame]:
        """Every picture in presentation order, its `pts` the log time of its access unit.

        FFmpeg's decoder reorders pictures and carries each packet's time to its picture.
        """
        decoder = av.CodecContext.create("h264", "r")
        decoder.thread_type = "AUTO"
        packets = (
            self._packet(payload, time_ns) for payload, time_ns in self._stream.access_units()
        )
        # A final None flushes the pictures the decoder still holds for reordering.
        for packet in itertools.chain(packets, [None]):
            yield from decoder.decode(packet)

    @staticmethod
    def _packet(payload: bytes, time_ns: int) -> av.Packet:
        packet = av.Packet(payload)
        packet.pts = time_ns
        return packet

    @staticmethod
    def _resized(picture: av.VideoFrame, width: int, height: int) -> np.ndarray:
        return picture.to_ndarray(width=width, height=height, format="rgb24")
