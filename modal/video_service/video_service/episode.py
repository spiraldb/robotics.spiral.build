"""One episode's H.264 Annex-B packets, decoded in display order."""

import itertools
import re
from collections.abc import Iterator

import av

# An Annex-B start code; its next byte is a NAL unit header.
START_CODE = re.compile(b"\x00\x00\x01")


class RejectedVideo(ValueError):
    """A video the service cannot answer for; the message says why."""


class EpisodeVideo:
    """Complete H.264 Annex-B access units in decode order, each with its presentation time in
    nanoseconds from the episode's first message."""

    def __init__(self, payloads: list[bytes], times_ns: list[int]):
        self._payloads = payloads
        self._times_ns = times_ns

    def frames(self) -> Iterator[av.VideoFrame]:
        """Every picture in display order, each carrying its packet's presentation time as `pts`.

        FFmpeg's decoder reorders pictures and carries each packet's time to its picture. A stream
        that decodes to fewer pictures than it codes is rejected once the last one is out, so a
        caller never answers for part of an episode.
        """
        decoder = av.CodecContext.create("h264", "r")
        # A final None flushes the pictures the decoder still holds for reordering.
        packets = itertools.chain(map(self._packet, self._payloads, self._times_ns), [None])
        decoded = 0
        try:
            for packet in packets:
                for frame in decoder.decode(packet):
                    decoded += 1
                    yield frame
        except av.FFmpegError as error:
            raise RejectedVideo(f"undecodable H.264: {error}") from error
        coded = sum(map(self._carries_picture, self._payloads))
        if decoded != coded:
            raise RejectedVideo(f"decoded {decoded} of the {coded} pictures in the H.264 stream")

    @staticmethod
    def _packet(payload: bytes, time_ns: int) -> av.Packet:
        packet = av.Packet(payload)
        packet.pts = time_ns
        return packet

    @staticmethod
    def _carries_picture(payload: bytes) -> bool:
        """Whether an access unit holds a coded slice (NAL types 1-5) rather than only parameter
        sets or SEI."""
        return any(
            (payload[start.end()] & 0x1F) in range(1, 6)
            for start in START_CODE.finditer(payload)
            if start.end() < len(payload)
        )
