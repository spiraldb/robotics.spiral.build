#!/usr/bin/env python3
"""PyTorch compute worker for a Rust-owned Spiral Robotics training run.

Rust owns Spiral, Arrow batch fetch, bounded prefetch, lifecycle, persistence,
and checkpoint publication. This process owns only Arrow-to-torch conversion,
the model/optimizer, metrics, and atomic local checkpoint artifacts.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO, Iterator

import numpy as np
import pyarrow as pa
import torch
from torch import nn


FRAME_BATCH = 1
FRAME_ERROR = 2
FRAME_EPOCH_END = 3
FRAME_END = 4
FRAME_HEADER = struct.Struct("<BQQQQQIB")


def emit(kind: str, **values: Any) -> None:
    print(json.dumps({"type": kind, **values}, separators=(",", ":")), flush=True)


def read_exact(stream: BinaryIO, length: int) -> bytes:
    chunks: list[bytes] = []
    remaining = length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise EOFError("the Rust data stream ended in the middle of a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


@dataclass
class Frame:
    kind: int
    payload: bytes
    epoch: int
    fetch_seconds: float
    source_bytes: int
    loader_seconds: float
    queue_depth: int
    normalized_json: bool


def frames(stream: BinaryIO) -> Iterator[Frame]:
    while True:
        header = stream.read(FRAME_HEADER.size)
        if not header:
            return
        if len(header) != FRAME_HEADER.size:
            header += read_exact(stream, FRAME_HEADER.size - len(header))
        kind, payload_size, epoch, fetch_ns, source_bytes, loader_ns, queue_depth, flags = (
            FRAME_HEADER.unpack(header)
        )
        payload = read_exact(stream, payload_size)
        yield Frame(
            kind=kind,
            payload=payload,
            epoch=epoch,
            fetch_seconds=float(fetch_ns) / 1_000_000_000.0,
            source_bytes=source_bytes,
            loader_seconds=float(loader_ns) / 1_000_000_000.0,
            queue_depth=queue_depth,
            normalized_json=bool(flags & 1),
        )


def arrow_primitive_tensor(array: pa.Array) -> torch.Tensor:
    if array.null_count:
        raise ValueError("training vectors cannot contain null values")
    try:
        return torch.from_dlpack(array)
    except (BufferError, RuntimeError, TypeError):
        values = array.to_numpy(zero_copy_only=True)
        return torch.from_numpy(values)


def vector_tensor(array: pa.Array, name: str) -> tuple[torch.Tensor, bool]:
    """Return [row, feature] float32 values and whether JSON caused a copy."""
    value_type = array.type
    if pa.types.is_fixed_size_list(value_type):
        if array.null_count:
            raise ValueError(f"{name} cannot contain null vectors")
        width = value_type.list_size
        # `values` is the unsliced child array. Account for the parent offset.
        child = array.values.slice(array.offset * width, len(array) * width)
        tensor = arrow_primitive_tensor(child).reshape(len(array), width)
        return tensor.to(dtype=torch.float32), False
    if pa.types.is_list(value_type) or pa.types.is_large_list(value_type):
        if array.null_count:
            raise ValueError(f"{name} cannot contain null vectors")
        offsets = array.offsets.to_numpy(zero_copy_only=True)
        widths = np.diff(offsets)
        if len(widths) == 0:
            return torch.empty((0, 0), dtype=torch.float32), False
        if not np.all(widths == widths[0]):
            raise ValueError(f"{name} vectors must all have the same width")
        start = int(offsets[0])
        length = int(offsets[-1]) - start
        child = array.values.slice(start, length)
        values = arrow_primitive_tensor(child)
        return values.reshape(len(array), int(widths[0])).to(torch.float32), False
    if pa.types.is_string(value_type) or pa.types.is_large_string(value_type):
        decoded = [json.loads(value) for value in array.to_pylist()]
        values = np.asarray(decoded, dtype=np.float32)
        if values.ndim != 2:
            raise ValueError(f"{name} JSON values must be equal-width numeric vectors")
        return torch.from_numpy(values), True
    if pa.types.is_floating(value_type) or pa.types.is_integer(value_type):
        return arrow_primitive_tensor(array).reshape(len(array), 1).to(torch.float32), False
    raise ValueError(
        f"{name} must be an Arrow numeric vector, list, fixed-size list, or JSON string; got {value_type}"
    )


@dataclass
class SourceBatch:
    state: torch.Tensor
    action: torch.Tensor
    episode_ids: list[str]
    stream_ids: list[str]
    sample_indices: list[int]
    fetch_seconds: float
    loader_seconds: float
    transfer_bytes_per_second: float
    queue_depth: int
    used_json_fallback: bool


def decode_batch(frame: Frame, request: dict[str, Any]) -> SourceBatch:
    reader = pa.ipc.open_stream(pa.BufferReader(frame.payload))
    record_batch = reader.read_next_batch()
    required = [
        "episode_id",
        "stream_id",
        "sample_index",
        request["state_column"],
        request["action_column"],
    ]
    missing = [name for name in required if record_batch.schema.get_field_index(name) < 0]
    if missing:
        raise ValueError(f"Rust sent an invalid training batch; missing {', '.join(missing)}")

    state, state_json = vector_tensor(
        record_batch.column(record_batch.schema.get_field_index(request["state_column"])),
        request["state_column"],
    )
    action, action_json = vector_tensor(
        record_batch.column(record_batch.schema.get_field_index(request["action_column"])),
        request["action_column"],
    )
    rows = record_batch.num_rows
    if state.shape[0] != rows or action.shape[0] != rows:
        raise ValueError("state and action columns returned different row counts")
    elapsed = max(frame.fetch_seconds, 1e-9)
    return SourceBatch(
        state=state,
        action=action,
        episode_ids=[str(value) for value in record_batch.column(
            record_batch.schema.get_field_index("episode_id")
        ).to_pylist()],
        stream_ids=[str(value) for value in record_batch.column(
            record_batch.schema.get_field_index("stream_id")
        ).to_pylist()],
        sample_indices=[int(value) for value in record_batch.column(
            record_batch.schema.get_field_index("sample_index")
        ).to_pylist()],
        fetch_seconds=frame.fetch_seconds,
        loader_seconds=frame.loader_seconds,
        transfer_bytes_per_second=float(frame.source_bytes) / elapsed,
        queue_depth=frame.queue_depth,
        used_json_fallback=frame.normalized_json or state_json or action_json,
    )


def validation_episode(episode_id: str) -> bool:
    """Assign a whole episode to validation using a stable 10% split."""
    digest = hashlib.blake2b(episode_id.encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(digest, "little") % 10 == 0


@dataclass
class SequenceWindow:
    state: torch.Tensor
    target: torch.Tensor
    validation: bool


class SequenceAssembler:
    """Build action chunks across Arrow batches without crossing episode gaps."""

    def __init__(self, chunk_size: int) -> None:
        self.chunk_size = chunk_size
        self.key: tuple[str, str] | None = None
        self.episode_id: str | None = None
        self.last_sample: int | None = None
        self.state: torch.Tensor | None = None
        self.action: torch.Tensor | None = None

    def _drain(self, final: bool) -> list[SequenceWindow]:
        if self.state is None or self.action is None or self.episode_id is None:
            return []
        rows = self.action.shape[0]
        usable = max(0, rows - self.chunk_size + 1)
        if usable == 0:
            if final:
                self.state = None
                self.action = None
            return []
        targets = self.action.unfold(0, self.chunk_size, 1).transpose(1, 2)
        result = [
            SequenceWindow(
                state=self.state[:usable],
                target=targets[:usable],
                validation=validation_episode(self.episode_id),
            )
        ]
        if final:
            self.state = None
            self.action = None
        else:
            keep = self.chunk_size - 1
            self.state = self.state[usable:] if keep else None
            self.action = self.action[usable:] if keep else None
        return result

    def feed(self, batch: SourceBatch) -> list[SequenceWindow]:
        windows: list[SequenceWindow] = []
        rows = len(batch.sample_indices)
        if rows == 0:
            return windows
        starts = [0]
        starts.extend(
            row
            for row in range(1, rows)
            if (
                batch.episode_ids[row] != batch.episode_ids[row - 1]
                or batch.stream_ids[row] != batch.stream_ids[row - 1]
                or batch.sample_indices[row] != batch.sample_indices[row - 1] + 1
            )
        )
        starts.append(rows)

        for span, (start, end) in enumerate(zip(starts, starts[1:])):
            episode_id = batch.episode_ids[start]
            stream_id = batch.stream_ids[start]
            sample_index = batch.sample_indices[start]
            key = (episode_id, stream_id)
            contiguous = self.key == key and (
                self.last_sample is None or sample_index == self.last_sample + 1
            )
            if self.key is not None and not contiguous:
                windows.extend(self._drain(final=True))
                self.state = None
                self.action = None
            if not contiguous:
                self.key = key
                self.episode_id = episode_id
                self.last_sample = None

            next_state = batch.state[start:end]
            next_action = batch.action[start:end]
            self.state = (
                next_state if self.state is None else torch.cat((self.state, next_state), dim=0)
            )
            self.action = (
                next_action
                if self.action is None
                else torch.cat((self.action, next_action), dim=0)
            )
            self.last_sample = batch.sample_indices[end - 1]
            windows.extend(self._drain(final=span + 1 < len(starts) - 1))
        return windows

    def finish(self) -> list[SequenceWindow]:
        windows = self._drain(final=True)
        self.key = None
        self.episode_id = None
        self.last_sample = None
        return windows


class StateAct(nn.Module):
    """Compact action-chunking transformer for low-dimensional robot state."""

    def __init__(self, state_dim: int, action_dim: int, chunk_size: int) -> None:
        super().__init__()
        self.state_dim = state_dim
        self.action_dim = action_dim
        width = 256
        self.state_projection = nn.Sequential(
            nn.LayerNorm(state_dim), nn.Linear(state_dim, width), nn.GELU()
        )
        self.action_queries = nn.Parameter(torch.randn(chunk_size, width) * 0.02)
        self.position = nn.Parameter(torch.randn(chunk_size + 1, width) * 0.01)
        layer = nn.TransformerEncoderLayer(
            d_model=width,
            nhead=8,
            dim_feedforward=1024,
            dropout=0.1,
            activation="gelu",
            batch_first=True,
            norm_first=True,
        )
        self.transformer = nn.TransformerEncoder(
            layer, num_layers=4, enable_nested_tensor=False
        )
        self.action_head = nn.Linear(width, action_dim)

    def forward(self, state: torch.Tensor) -> torch.Tensor:
        batch = state.shape[0]
        state_token = self.state_projection(state).unsqueeze(1)
        queries = self.action_queries.unsqueeze(0).expand(batch, -1, -1)
        tokens = torch.cat((state_token, queries), dim=1) + self.position
        return self.action_head(self.transformer(tokens)[:, 1:])


def choose_device(requested: str) -> torch.device:
    name = requested.strip().lower()
    if name == "auto":
        if torch.cuda.is_available():
            name = "cuda"
        elif hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
            name = "mps"
        else:
            name = "cpu"
    device = torch.device(name)
    if device.type == "cuda" and not torch.cuda.is_available():
        raise RuntimeError("CUDA was requested but is not available to PyTorch")
    if device.type == "mps" and not torch.backends.mps.is_available():
        raise RuntimeError("MPS was requested but is not available to PyTorch")
    return device


def resource_metrics(
    device: torch.device, wall_delta: float, cpu_delta: float
) -> tuple[float | None, int | None, float]:
    gpu_utilization = None
    gpu_memory = None
    if device.type == "cuda":
        gpu_memory = int(torch.cuda.memory_allocated(device))
        try:
            gpu_utilization = float(torch.cuda.utilization(device))
        except (AttributeError, RuntimeError):
            pass
    elif device.type == "mps":
        try:
            gpu_memory = int(torch.mps.current_allocated_memory())
        except (AttributeError, RuntimeError):
            pass
    cores = max(os.cpu_count() or 1, 1)
    cpu = min(100.0, max(0.0, cpu_delta / max(wall_delta, 1e-9) * 100.0 / cores))
    return gpu_utilization, gpu_memory, cpu


def save_checkpoint(
    run_directory: Path,
    config: dict[str, Any],
    model: nn.Module,
    optimizer: torch.optim.Optimizer,
    step: int,
    validation_loss: float | None,
) -> None:
    file_name = f"checkpoint-{step}.pt"
    target = run_directory / file_name
    temporary = run_directory / f".{file_name}.partial"
    torch.save(
        {
            "format": "spiral_robotics.state_act.v1",
            "run_id": config["run_id"],
            "step": step,
            "request": config["request"],
            "source": config["source"],
            "model": model.state_dict(),
            "optimizer": optimizer.state_dict(),
        },
        temporary,
    )
    os.replace(temporary, target)
    created_ms = int(time.time() * 1000)
    emit(
        "checkpoint_ready",
        step=step,
        created_ms=created_ms,
        validation_loss=validation_loss,
        file_name=file_name,
        byte_size=target.stat().st_size,
    )


def train(config: dict[str, Any], stream: BinaryIO) -> None:
    request = config["request"]
    torch.manual_seed(int(request["seed"]))
    np.random.seed(int(request["seed"]) % (2**32))
    device = choose_device(request["device"])
    run_directory = Path.cwd().resolve()
    stop_file = Path(config["stop_file"])
    assembler = SequenceAssembler(int(request["chunk_size"]))
    model: StateAct | None = None
    optimizer: torch.optim.Optimizer | None = None
    scheduler: torch.optim.lr_scheduler.LRScheduler | None = None
    step = 0
    last_validation: float | None = None
    pending_validation: float | None = None
    json_notice_sent = False
    start_wall = time.perf_counter()
    metric_wall = start_wall
    metric_cpu = time.process_time()
    examples_since_metric = 0
    metric_every = 10
    validation_every = max(25, min(250, int(request["checkpoint_every"]) // 4))
    last_validation_step = -1
    last_checkpoint = 0
    last_train_loss: float | None = None
    last_transfer = 0.0
    last_queue_depth = 0
    loader_stall_pending = False
    epoch_training_windows = 0
    epoch_windows = 0
    stop_requested = False

    def ensure_model(window: SequenceWindow) -> None:
        nonlocal model, optimizer, scheduler
        if model is None:
            learning_rate = float(request["learning_rate"])
            model = StateAct(
                window.state.shape[1],
                window.target.shape[2],
                int(request["chunk_size"]),
            ).to(device)
            optimizer = torch.optim.AdamW(
                model.parameters(), lr=learning_rate, weight_decay=1e-4
            )
            scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(
                optimizer, T_max=max(int(request["steps"]), 1), eta_min=min(1e-6, learning_rate)
            )
            emit(
                "log",
                message=(
                    f"ACT initialized: state={window.state.shape[1]}, "
                    f"action={window.target.shape[2]}, chunk={request['chunk_size']}; "
                    "validation is isolated by whole episode."
                ),
            )
        elif (
            window.state.shape[1] != model.state_dim
            or window.target.shape[2] != model.action_dim
        ):
            raise ValueError(
                "the query mixed incompatible state/action widths; filter to one compatible robot stream"
            )

    def evaluate(window: SequenceWindow) -> float:
        assert model is not None
        count = min(window.state.shape[0], int(request["batch_size"]))
        state = window.state[:count].to(device)
        target = window.target[:count].to(device)
        model.eval()
        with torch.inference_mode():
            return float(nn.functional.smooth_l1_loss(model(state), target).item())

    def process(window: SequenceWindow) -> None:
        nonlocal step, last_validation, pending_validation, last_validation_step
        nonlocal examples_since_metric, metric_wall, metric_cpu, last_checkpoint
        nonlocal last_train_loss, stop_requested, epoch_training_windows
        nonlocal loader_stall_pending
        ensure_model(window)
        assert model is not None and optimizer is not None and scheduler is not None

        if window.validation:
            if step == 0 or step - last_validation_step >= validation_every:
                last_validation = evaluate(window)
                pending_validation = last_validation
                last_validation_step = step
            return

        epoch_training_windows += window.state.shape[0]
        order = torch.randperm(window.state.shape[0])
        for offset in range(0, order.numel(), int(request["batch_size"])):
            if step >= int(request["steps"]) or stop_file.exists():
                stop_requested = stop_requested or stop_file.exists()
                return
            indices = order[offset : offset + int(request["batch_size"])]
            batch_state = window.state[indices].to(device)
            batch_action = window.target[indices].to(device)
            model.train()
            optimizer.zero_grad(set_to_none=True)
            predicted = model(batch_state)
            loss = nn.functional.smooth_l1_loss(predicted, batch_action)
            loss.backward()
            nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            scheduler.step()
            step += 1
            last_train_loss = float(loss.item())
            examples_since_metric += int(batch_state.shape[0])

            validation_for_point = pending_validation
            should_emit = (
                step == 1
                or step % metric_every == 0
                or step == int(request["steps"])
                or validation_for_point is not None
                or loader_stall_pending
            )
            if should_emit:
                now_wall = time.perf_counter()
                now_cpu = time.process_time()
                wall_delta = max(now_wall - metric_wall, 1e-9)
                gpu, gpu_memory, cpu = resource_metrics(
                    device, wall_delta, now_cpu - metric_cpu
                )
                events = []
                if validation_for_point is not None:
                    events.append("validation")
                if loader_stall_pending:
                    events.append("loader stall")
                emit(
                    "metric",
                    step=step,
                    train_loss=last_train_loss,
                    validation_loss=validation_for_point,
                    learning_rate=float(optimizer.param_groups[0]["lr"]),
                    gpu_utilization_pct=gpu,
                    gpu_memory_bytes=gpu_memory,
                    cpu_utilization_pct=cpu,
                    transfer_bytes_per_second=last_transfer,
                    examples_per_second=examples_since_metric / wall_delta,
                    queue_depth=last_queue_depth,
                    elapsed_seconds=now_wall - start_wall,
                    event=" · ".join(events) or None,
                )
                pending_validation = None
                loader_stall_pending = False
                metric_wall = now_wall
                metric_cpu = now_cpu
                examples_since_metric = 0

            if step % int(request["checkpoint_every"]) == 0:
                save_checkpoint(
                    run_directory, config, model, optimizer, step, last_validation
                )
                last_checkpoint = step

    emit(
        "started",
        message=f"Rust is streaming Spiral Arrow batches into PyTorch on {device.type}.",
    )
    for frame in frames(stream):
        if frame.kind == FRAME_ERROR:
            raise RuntimeError(frame.payload.decode("utf-8", errors="replace"))
        if frame.kind == FRAME_END:
            stop_requested = stop_requested or stop_file.exists()
            break
        if frame.kind == FRAME_EPOCH_END:
            for window in assembler.finish():
                epoch_windows += window.state.shape[0]
                process(window)
            if step < int(request["steps"]) and not stop_requested:
                if epoch_windows == 0:
                    raise RuntimeError(
                        "the Spiral query produced no contiguous sequence as long as the action chunk"
                    )
                if epoch_training_windows == 0:
                    raise RuntimeError(
                        "the episode split produced validation data but no training episodes"
                    )
            epoch_windows = 0
            epoch_training_windows = 0
            continue
        if frame.kind != FRAME_BATCH:
            raise RuntimeError(f"Rust sent unknown training frame kind {frame.kind}")

        source = decode_batch(frame, request)
        last_transfer = source.transfer_bytes_per_second
        last_queue_depth = source.queue_depth
        loader_stall_pending = loader_stall_pending or source.loader_seconds >= 2.0
        if source.used_json_fallback and not json_notice_sent:
            emit(
                "log",
                message=(
                    "Spiral returned JSON vectors. Rust normalized this compatibility "
                    "path to typed float32 Arrow before the PyTorch boundary."
                ),
            )
            json_notice_sent = True
        for window in assembler.feed(source):
            epoch_windows += window.state.shape[0]
            process(window)
        if step >= int(request["steps"]) or stop_requested:
            break

    if model is None or optimizer is None or last_train_loss is None:
        if stop_requested:
            emit("stopped", message="Stopped before the first optimizer step.")
            return
        raise RuntimeError("the Spiral query did not produce a usable training batch")

    if last_checkpoint != step:
        save_checkpoint(run_directory, config, model, optimizer, step, last_validation)
    if stop_requested or stop_file.exists():
        emit(
            "stopped",
            message=f"Stopped after step {step}; the final checkpoint was published to Spiral.",
        )
    elif step >= int(request["steps"]):
        emit(
            "completed",
            message=f"Completed {step} steps; the final checkpoint was published to Spiral.",
        )
    else:
        raise RuntimeError("the Rust data stream ended before training completed")


def self_test() -> None:
    assert FRAME_HEADER.size == 46
    fixed = pa.array(
        [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]],
        type=pa.list_(pa.float32(), 2),
    ).slice(1, 2)
    fixed_tensor, copied = vector_tensor(fixed, "fixed")
    assert not copied and fixed_tensor.tolist() == [[3.0, 4.0], [5.0, 6.0]]

    variable = pa.array(
        [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]],
        type=pa.list_(pa.float32()),
    ).slice(1, 2)
    variable_tensor, copied = vector_tensor(variable, "variable")
    assert not copied and variable_tensor.tolist() == [[3.0, 4.0], [5.0, 6.0]]

    def source(start: int, rows: int) -> SourceBatch:
        values = torch.arange(start, start + rows, dtype=torch.float32).reshape(-1, 1)
        return SourceBatch(
            state=values,
            action=values,
            episode_ids=["episode-a"] * rows,
            stream_ids=["arm"] * rows,
            sample_indices=list(range(start, start + rows)),
            fetch_seconds=0.01,
            loader_seconds=0.01,
            transfer_bytes_per_second=1.0,
            queue_depth=1,
            used_json_fallback=False,
        )

    assembler = SequenceAssembler(3)
    first = assembler.feed(source(0, 2))
    second = assembler.feed(source(2, 3))
    final = assembler.finish()
    assert not first
    assert sum(window.state.shape[0] for window in second + final) == 3
    targets = torch.cat([window.target for window in second + final], dim=0)
    assert targets[:, :, 0].tolist() == [[0.0, 1.0, 2.0], [1.0, 2.0, 3.0], [2.0, 3.0, 4.0]]
    assert validation_episode("same-episode") == validation_episode("same-episode")

    model = StateAct(state_dim=2, action_dim=2, chunk_size=3)
    output = model(torch.zeros((2, 2), dtype=torch.float32))
    assert output.shape == (2, 3, 2)


def main() -> int:
    parser = argparse.ArgumentParser(description="Spiral Robotics isolated PyTorch worker")
    parser.add_argument("--config", type=Path)
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.probe:
        print(f"PyTorch {torch.__version__} · Arrow {pa.__version__}")
        return 0
    if args.self_test:
        self_test()
        print("training worker self-test passed")
        return 0
    if args.config is None:
        parser.error("--config is required unless --probe or --self-test is used")
    with args.config.open("r", encoding="utf-8") as handle:
        config = json.load(handle)
    try:
        train(config, sys.stdin.buffer)
    except Exception as error:
        emit("failed", message=f"{type(error).__name__}: {error}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
