"""One episode's H.264 Annex-B packets decoded to display-order RGB pictures, as spiral-video decodes them."""

import itertools
import re
from collections.abc import Iterator

import av
import numpy as np

# An Annex-B start code; its next byte is a NAL unit header.
START_CODE = re.compile(b"\x00\x00\x01")


class RejectedVideo(ValueError):
    """A video the pinned checkpoint cannot score; the message says why."""


class PacketVideo:
    """Complete H.264 Annex-B access units in decode order, each with its presentation time in nanoseconds."""

    def __init__(self, payloads: list[bytes], times_ns: list[int]):
        self._payloads = payloads
        self._times_ns = times_ns

    def pictures(self) -> Iterator[tuple[int, np.ndarray]]:
        """Every picture in display order: its presentation time and its height x width x 3 RGB24 pixels.

        FFmpeg's decoder reorders pictures and carries each packet's time to its picture. The colour
        conversion is spiral-video's: swscale at the decoded size with FAST_BILINEAR, using the matrix
        and range the stream signals, so an untagged stream converts as BT.601 limited range.
        """
        decoder = av.CodecContext.create("h264", "r")
        # A final None flushes the pictures the decoder still holds for reordering.
        packets = itertools.chain(map(self._packet, self._payloads, self._times_ns), [None])
        decoded = 0
        try:
            for packet in packets:
                for frame in decoder.decode(packet):
                    decoded += 1
                    yield frame.pts, frame.to_ndarray(format="rgb24", interpolation="FAST_BILINEAR")
        except av.FFmpegError as error:
            raise RejectedVideo(f"undecodable H.264: {error}") from error
        pictures = sum(map(self._carries_picture, self._payloads))
        if decoded != pictures:
            raise RejectedVideo(f"decoded {decoded} of the {pictures} pictures in the H.264 stream")

    @staticmethod
    def _packet(payload: bytes, time_ns: int) -> av.Packet:
        packet = av.Packet(payload)
        packet.pts = time_ns
        return packet

    @staticmethod
    def _carries_picture(payload: bytes) -> bool:
        """Whether an access unit holds a coded slice (NAL types 1-5) rather than only parameter sets or SEI."""
        return any(
            (payload[start.end()] & 0x1F) in range(1, 6)
            for start in START_CODE.finditer(payload)
            if start.end() < len(payload)
        )
