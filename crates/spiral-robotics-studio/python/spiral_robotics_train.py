#!/usr/bin/env python3
"""Isolated vision-conditioned ACT training worker.

The worker opens a read-only Spiral session, reads its small startup metadata
as Arrow, and consumes every per-step sample payload through ``to_torch()``.
Rust owns lifecycle, history, and checkpoint publication; this process owns
sample preparation and PyTorch compute.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any

import numpy as np
import pyarrow as pa
import spiraldb as sp
import torch
from torch import nn

from spiral_robotics_data import (
    SpiralActLoader,
    TrainingCancelled,
    self_test as data_self_test,
)
from spiral_robotics_policy import ActPolicy, act_loss, self_test as policy_self_test


def emit(kind: str, **values: Any) -> None:
    print(json.dumps({"type": kind, **values}, separators=(",", ":")), flush=True)


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


def parse_mps_device_utilization(output: str) -> float | None:
    match = re.search(r'"Device Utilization %"\s*=\s*(\d+(?:\.\d+)?)', output)
    if match is None:
        return None
    return min(100.0, max(0.0, float(match.group(1))))


def mps_device_utilization() -> float | None:
    if sys.platform != "darwin":
        return None
    try:
        result = subprocess.run(
            ["/usr/sbin/ioreg", "-r", "-d", "1", "-w", "0", "-c", "AGXAccelerator"],
            check=False,
            capture_output=True,
            text=True,
            timeout=1.0,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return parse_mps_device_utilization(result.stdout) if result.returncode == 0 else None


def mps_memory_bytes() -> int | None:
    for method_name in ("driver_allocated_memory", "current_allocated_memory"):
        method = getattr(torch.mps, method_name, None)
        if method is not None:
            try:
                return int(method())
            except RuntimeError:
                pass
    return None


def resource_metrics(
    device: torch.device, wall_delta: float, cpu_delta: float
) -> tuple[float | None, int | None, float]:
    gpu_utilization = None
    gpu_memory = None
    if device.type == "cuda":
        gpu_memory = int(torch.cuda.memory_reserved(device))
        try:
            gpu_utilization = float(torch.cuda.utilization(device))
        except (AttributeError, ImportError, RuntimeError):
            pass
    elif device.type == "mps":
        gpu_utilization = mps_device_utilization()
        gpu_memory = mps_memory_bytes()
    cpu = max(0.0, cpu_delta / max(wall_delta, 1e-9) * 100.0)
    return gpu_utilization, gpu_memory, cpu


def save_checkpoint(
    run_directory: Path,
    config: dict[str, Any],
    loader: SpiralActLoader,
    model: nn.Module,
    step: int,
    validation_loss: float | None,
) -> None:
    file_name = f"checkpoint-{step}.pt"
    target = run_directory / file_name
    temporary = run_directory / f".{file_name}.partial"
    torch.save(
        {
            "format": "spiral_robotics.act_cvae.v4",
            "artifact": "inference",
            "run_id": config["run_id"],
            "step": step,
            "request": config["request"],
            "source": {
                "project": config["spiral_project"],
                "dataset": config["spiral_dataset"],
            },
            "data_contract": {
                "value_space": "centered_normalized",
                "robot_type": loader.robot_type,
                "joint_names": loader.joint_names,
                "camera_streams": loader.camera_streams,
                "sample_hz": float(loader.sample_hz),
                "image_layout": "chw",
                "image_normalization": "imagenet",
                "frame_alignment": "fixed_fps",
                "spiral_schema_version": 5,
            },
            "runtime": {
                "torch": torch.__version__,
                "spiraldb": sp.__version__,
                "pyarrow": pa.__version__,
            },
            "normalization": loader.stats.as_checkpoint(),
            "model": model.state_dict(),
            "random_state": {
                "torch": torch.random.get_rng_state(),
                "numpy": np.random.get_state(),
                "loader_train": loader.train_random.bit_generator.state,
                "loader_validation": loader.validation_random.bit_generator.state,
            },
        },
        temporary,
    )
    os.replace(temporary, target)
    emit(
        "checkpoint_ready",
        step=step,
        created_ms=int(time.time() * 1_000),
        validation_loss=validation_loss,
        file_name=file_name,
        byte_size=target.stat().st_size,
    )


@torch.inference_mode()
def evaluate(
    loader: SpiralActLoader,
    model: ActPolicy,
    batch_size: int,
    kl_weight: float,
) -> float:
    batch = loader.fetch(batch_size, validation=True)
    model.eval()
    predicted, predicted_pad, mean, log_variance = model(
        batch.state, batch.images, batch.actions, batch.action_is_pad
    )
    loss, _ = act_loss(
        predicted,
        predicted_pad,
        batch.actions,
        batch.action_is_pad,
        mean,
        log_variance,
        kl_weight,
    )
    return float(loss)


def train(config: dict[str, Any]) -> None:
    request = config["request"]
    seed = int(request["seed"])
    torch.manual_seed(seed)
    np.random.seed(seed % (2**32))
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(seed)
    device = choose_device(request["device"])
    run_directory = Path.cwd().resolve()
    stop_file = Path(config["stop_file"])

    emit("log", message="Opening the Spiral Python relation and indexing eligible ACT anchors…")
    loader = SpiralActLoader(config, device)
    act_config = request["act"]
    model = ActPolicy(
        state_dim=loader.state_dim,
        action_dim=loader.action_dim,
        camera_count=len(loader.camera_streams),
        chunk_size=int(request["chunk_size"]),
        config=act_config,
    ).to(device)
    optimizer = torch.optim.AdamW(
        model.parameters(),
        lr=float(request["learning_rate"]),
        weight_decay=1.0e-4,
    )
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(
        optimizer,
        T_max=max(int(request["steps"]), 1),
        eta_min=min(1.0e-6, float(request["learning_rate"])),
    )
    parameter_count = sum(parameter.numel() for parameter in model.parameters())
    emit(
        "started",
        message=(
            f"Vision ACT is running on {device.type}: {loader.summary()}; "
            f"{parameter_count / 1_000_000:.1f}M parameters."
        ),
    )
    emit(
        "log",
        message=(
            f"ACT inputs: state={loader.state_dim}, action={loader.action_dim}, "
            f"chunk={request['chunk_size']}, cameras={', '.join(loader.camera_streams)}."
        ),
    )

    steps = int(request["steps"])
    batch_size = int(request["batch_size"])
    checkpoint_every = int(request["checkpoint_every"])
    validation_every = max(25, min(250, checkpoint_every // 4))
    kl_weight = float(act_config["kl_weight"])
    metric_every = 10
    start_wall = time.perf_counter()
    metric_wall = start_wall
    metric_cpu = time.process_time()
    examples_since_metric = 0
    last_validation: float | None = None
    last_checkpoint = 0
    last_loss: float | None = None
    completed_steps = 0
    stop_requested = False

    for step in range(1, steps + 1):
        if stop_file.exists():
            stop_requested = True
            break
        try:
            batch = loader.fetch(batch_size, validation=False)
        except TrainingCancelled:
            stop_requested = True
            break
        model.train()
        optimizer.zero_grad(set_to_none=True)
        predicted, predicted_pad, mean, log_variance = model(
            batch.state, batch.images, batch.actions, batch.action_is_pad
        )
        loss, parts = act_loss(
            predicted,
            predicted_pad,
            batch.actions,
            batch.action_is_pad,
            mean,
            log_variance,
            kl_weight,
        )
        if not loss.isfinite():
            raise RuntimeError(f"ACT loss became non-finite at step {step}")
        loss.backward()
        nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        optimizer.step()
        scheduler.step()
        last_loss = float(loss.detach())
        completed_steps = step
        examples_since_metric += batch_size

        event_parts: list[str] = []
        validation_for_point = None
        if loader.validation_anchors and step % validation_every == 0:
            try:
                last_validation = evaluate(loader, model, batch_size, kl_weight)
            except TrainingCancelled:
                stop_requested = True
                break
            validation_for_point = last_validation
            event_parts.append("validation")
        if batch.metrics.seconds >= 2.0:
            event_parts.append("loader stall")

        if step == 1 or step % metric_every == 0 or step == steps or event_parts:
            now_wall = time.perf_counter()
            now_cpu = time.process_time()
            wall_delta = max(now_wall - metric_wall, 1.0e-9)
            gpu, gpu_memory, cpu = resource_metrics(device, wall_delta, now_cpu - metric_cpu)
            emit(
                "metric",
                step=step,
                train_loss=last_loss,
                validation_loss=validation_for_point,
                learning_rate=float(optimizer.param_groups[0]["lr"]),
                gpu_utilization_pct=gpu,
                gpu_memory_bytes=gpu_memory,
                cpu_utilization_pct=cpu,
                transfer_bytes_per_second=batch.metrics.source_bytes / max(batch.metrics.seconds, 1.0e-9),
                examples_per_second=examples_since_metric / wall_delta,
                queue_depth=2,
                elapsed_seconds=now_wall - start_wall,
                event=" · ".join(event_parts) or None,
            )
            if step == 1:
                emit(
                    "log",
                    message=(
                        f"Loss components at step 1: L1={parts['l1']:.4g}, "
                        f"KL={parts['kl']:.4g}, pad={parts['pad']:.4g}."
                    ),
                )
            metric_wall = now_wall
            metric_cpu = now_cpu
            examples_since_metric = 0

        if step % checkpoint_every == 0:
            save_checkpoint(
                run_directory,
                config,
                loader,
                model,
                step,
                last_validation,
            )
            last_checkpoint = step

    if last_loss is None:
        emit("stopped", message="Stopped before the first optimizer step.")
        return
    if last_checkpoint != completed_steps:
        save_checkpoint(
            run_directory,
            config,
            loader,
            model,
            completed_steps,
            last_validation,
        )
    if stop_requested or stop_file.exists():
        emit(
            "stopped",
            message=f"Stopped after step {completed_steps}; the final checkpoint was published to Spiral.",
        )
    else:
        emit(
            "completed",
            message=f"Completed {completed_steps} steps; the final checkpoint was published to Spiral.",
        )


def self_test() -> None:
    data_self_test()
    policy_self_test()
    assert parse_mps_device_utilization('"PerformanceStatistics" = {"Device Utilization %"=35}') == 35.0
    assert resource_metrics(torch.device("cpu"), 0.5, 2.5) == (None, None, 500.0)


def main() -> int:
    parser = argparse.ArgumentParser(description="Spiral Robotics vision ACT worker")
    parser.add_argument("--config", type=Path)
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.probe:
        relation = sp.Spiral().values(
            pa.table(
                {
                    "value": pa.array(
                        [[1.0, 2.0]],
                        type=pa.list_(pa.field("item", pa.float32(), nullable=False), 2),
                    )
                }
            )
        )
        value = next(iter(relation.to_torch(device="cpu", prefetch=1)))["value"]
        if tuple(value.shape) != (1, 2):
            raise RuntimeError("Spiral to_torch() probe returned an unexpected tensor shape")
        print(
            f"PyTorch {torch.__version__} · SpiralDB {sp.__version__} · "
            f"Arrow {pa.__version__} · to_torch verified"
        )
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
        train(config)
    except Exception as error:
        traceback.print_exc(file=sys.stderr)
        emit("failed", message=f"{type(error).__name__}: {error}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
