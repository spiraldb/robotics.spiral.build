"""The device a run's job trains on, and what each step on it cost: shared by every model family's
session, so their metrics read alike."""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any

import psutil
import torch


@dataclass(frozen=True)
class StepResult:
    loss: float
    device_copy_ms: float
    model_ms: float


class TrainingDevice:
    """One device, and the job's process's use of it and of the host."""

    def __init__(self, device: torch.device) -> None:
        self._device = device
        self._process = psutil.Process()
        self._process.cpu_percent(interval=None)

    @classmethod
    def available(cls) -> TrainingDevice:
        """The first CUDA device when there is one, else the CPU."""
        return cls(torch.device("cuda:0" if torch.cuda.is_available() else "cpu"))

    @property
    def torch_device(self) -> torch.device:
        return self._device

    @property
    def name(self) -> str:
        if self._device.type == "cuda":
            index = self._device.index or 0
            return f"CUDA:{index} ({torch.cuda.get_device_name(index)})"
        return self._device.type.upper()

    def finished_ms(self, started: float) -> float:
        """Milliseconds from `started`, a `time.perf_counter()` reading, until the device has
        finished all the work queued on it."""
        if self._device.type == "cuda":
            torch.cuda.synchronize(self._device)
        return (time.perf_counter() - started) * 1_000.0

    def telemetry(self) -> dict[str, Any]:
        return {
            "cpu_percent": float(self._process.cpu_percent(interval=None)),
            "gpu_percent": self._gpu_percent(),
            "memory_bytes": int(self._process.memory_info().rss),
        }

    def _gpu_percent(self) -> float | None:
        if self._device.type != "cuda":
            return None
        try:
            return float(torch.cuda.utilization(self._device))
        except (AttributeError, ImportError, RuntimeError, ValueError):
            return None
