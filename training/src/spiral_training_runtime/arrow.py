"""Rollout inference inputs: one Arrow IPC record batch of FixedSizeList tensors."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
import pyarrow as pa

from .config import PolicyConfig
from .errors import RuntimeRequestError


@dataclass(frozen=True)
class InferenceArrays:
    owner: pa.RecordBatch | None
    state: np.ndarray
    images: tuple[np.ndarray, ...]


def decode_inference_input(body: bytes, config: PolicyConfig) -> InferenceArrays:
    batch = _one_batch(body)
    if batch.num_rows != 1:
        raise RuntimeRequestError(
            f"Arrow inference input has {batch.num_rows} rows, expected exactly one"
        )
    state = _fixed_values(batch, "state", config.state_dimensions, pa.float32())
    image_width = 3 * config.image_height * config.image_width
    images = tuple(
        _fixed_values(batch, f"image_{index}", image_width, pa.uint8())
        for index in range(len(config.camera_streams))
    )
    return InferenceArrays(owner=batch, state=state, images=images)


def _one_batch(body: bytes) -> pa.RecordBatch:
    if not body:
        raise RuntimeRequestError("Arrow IPC request body is empty")
    try:
        with pa.ipc.open_stream(pa.py_buffer(body)) as reader:
            batches = list(reader)
    except (pa.ArrowException, OSError, ValueError) as error:
        raise RuntimeRequestError(f"invalid Arrow IPC stream: {error}") from error
    if len(batches) != 1:
        raise RuntimeRequestError("Arrow IPC stream must contain exactly one record batch")
    return batches[0]


def _fixed_values(
    batch: pa.RecordBatch,
    name: str,
    width: int,
    value_type: pa.DataType,
) -> np.ndarray:
    index = batch.schema.get_field_index(name)
    if index < 0:
        raise RuntimeRequestError(f"missing FixedSizeList Arrow tensor {name!r}")
    column = batch.column(index)
    if not pa.types.is_fixed_size_list(column.type):
        raise RuntimeRequestError(f"Arrow tensor {name!r} must be FixedSizeList")
    if column.type.list_size != width or column.type.value_type != value_type:
        raise RuntimeRequestError(
            f"Arrow tensor {name!r} must have shape [{batch.num_rows}, {width}] "
            f"with {value_type}, found {column.type}"
        )
    if column.null_count or column.values.null_count:
        raise RuntimeRequestError(f"Arrow tensor {name!r} cannot contain null values")
    start = column.offset * width
    values = column.values.slice(start, batch.num_rows * width)
    try:
        array = values.to_numpy(zero_copy_only=True, writable=False)
    except pa.ArrowInvalid as error:
        raise RuntimeRequestError(f"Arrow tensor {name!r} is not contiguous: {error}") from error
    return array.reshape(batch.num_rows, width)
