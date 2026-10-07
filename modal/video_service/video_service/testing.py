"""What the services' tests send: a small synthetic episode, as a request body."""

from fractions import Fraction

import av
import numpy as np
import pyarrow as pa

from video_service.service import ARROW_STREAM

AUTHORIZED = {"Authorization": "Bearer secret", "Content-Type": ARROW_STREAM}


class SyntheticEpisode:
    """A small libx264 Annex-B stream with B-frames, so decode order differs from display order. Each
    picture is one grey level, rising picture by picture."""

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

    def packets(self, metadata: dict[str, str | None]) -> pa.Table:
        """The episode as a request table with `metadata`; a None value leaves its key out."""
        return pa.table(
            {
                "payload": pa.array(self.payloads, pa.binary()),
                "decode_index": pa.array(range(len(self.payloads)), pa.int64()),
                "pts_ns": pa.array(self.times_ns, pa.int64()),
            }
        ).replace_schema_metadata({key: value for key, value in metadata.items() if value is not None})

    def request(self, metadata: dict[str, str | None]) -> bytes:
        """The episode as an Arrow IPC request body with `metadata`."""
        return self.body(self.packets(metadata))

    @staticmethod
    def body(table: pa.Table) -> bytes:
        sink = pa.BufferOutputStream()
        with pa.ipc.new_stream(sink, table.schema) as writer:
            writer.write_table(table)
        return sink.getvalue().to_pybytes()
