"""Uploaded streams: one camera's H.264 access units, or one joint stream's values, each an Arrow
IPC stream whose rows are in recorded order. See docs/training.md#streams.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import numpy as np
import pyarrow as pa

from .errors import RuntimeRequestError

CAMERA_METADATA = {b"codec": b"h264", b"framing": b"annex_b"}


class CameraStream:
    """One camera's H.264 Annex-B access units in decode order, each with its log time."""

    def __init__(self, table: pa.Table) -> None:
        schema = table.schema
        if (
            table.column_names != ["log_time_ns", "payload"]
            or schema.field("log_time_ns").type != pa.int64()
            or schema.field("payload").type != pa.binary()
            or table["log_time_ns"].null_count
            or table["payload"].null_count
        ):
            raise RuntimeRequestError(
                "a camera stream has non-null `log_time_ns` int64 and `payload` binary columns",
                422,
            )
        metadata = schema.metadata or {}
        if any(metadata.get(key) != value for key, value in CAMERA_METADATA.items()):
            raise RuntimeRequestError("a camera stream carries codec=h264, framing=annex_b", 422)
        self._table = table

    @classmethod
    def read(cls, path: Path) -> CameraStream:
        return cls(_read_arrow_stream(path))

    def access_units(self) -> Iterator[tuple[bytes, int]]:
        """Each access unit's bytes and log time in nanoseconds, in decode order."""
        times = self._table["log_time_ns"].to_pylist()
        return zip(self._table["payload"].to_pylist(), times, strict=True)


class ValueStream:
    """One joint stream's values in log-time order, rows with a null value skipped."""

    def __init__(self, table: pa.Table) -> None:
        names = table.column_names
        value_names = names[1:]
        if (
            not names
            or names[0] != "log_time_ns"
            or table.schema.field(0).type != pa.int64()
            or table["log_time_ns"].null_count
            or not value_names
            or value_names != [f"v{index}" for index in range(len(value_names))]
            or any(table.schema.field(name).type != pa.float64() for name in value_names)
        ):
            raise RuntimeRequestError(
                "a value stream has a non-null `log_time_ns` int64 column, then float64 "
                "columns v0, v1, …",
                422,
            )
        complete = np.logical_and.reduce(
            [table[name].is_valid().to_numpy(zero_copy_only=False) for name in value_names]
        )
        self._times_ns = table["log_time_ns"].to_numpy()[complete]
        self._values = np.column_stack(
            [table[name].to_numpy(zero_copy_only=False)[complete] for name in value_names]
        )
        if not len(self._times_ns):
            raise RuntimeRequestError("a value stream has no row without a null value", 422)
        if np.any(np.diff(self._times_ns) < 0):
            raise RuntimeRequestError("a value stream's rows are not in log-time order", 422)

    @classmethod
    def read(cls, path: Path) -> ValueStream:
        return cls(_read_arrow_stream(path))

    @property
    def columns(self) -> int:
        return self._values.shape[1]

    def held(self, ticks_ns: np.ndarray) -> np.ndarray:
        """The latest values at or before each tick, the first values before the first row."""
        latest = np.searchsorted(self._times_ns, ticks_ns, side="right") - 1
        return self._values[np.maximum(latest, 0)]


class UploadedStream:
    @staticmethod
    def check(path: Path) -> None:
        """Rejects a file that is neither kind of stream, before it is stored under its key."""
        table = _read_arrow_stream(path)
        if "payload" in table.column_names:
            CameraStream(table)
        else:
            ValueStream(table)


def _read_arrow_stream(path: Path) -> pa.Table:
    try:
        with pa.OSFile(str(path)) as source:
            return pa.ipc.open_stream(source).read_all()
    except pa.ArrowInvalid as error:
        raise RuntimeRequestError(f"not an Arrow IPC stream: {error}", 422) from error
