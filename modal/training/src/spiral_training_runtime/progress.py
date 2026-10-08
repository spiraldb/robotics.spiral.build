"""The progress model: WARP-RM's progress head trained from scratch on one camera's pictures by
upstream's sim recipe, and checkpointed in the layout Score progress's `TemporalModel` loads. See
docs/training.md#checkpoints.
"""

from __future__ import annotations

import json
import random
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import torch
from safetensors.torch import save as save_safetensors
from warp_progress.artifacts import BACKBONE_REPOSITORY, BACKBONE_REVISION
from warp_progress.warp import PictureEncoder

from .device import StepResult, TrainingDevice
from .family import PreparedRun
from .progress_embeddings import CameraEmbeddings
from .progress_recipe import (
    ABLATION,
    CROP_MODE,
    FEATURE_STRIDE,
    FPS,
    GRADIENT_CLIP,
    IMAGE_SIZE,
    SOURCE_STANDARD_STRIDE,
    STANDARD_WINDOW_FRAMES,
    WARMUP_STEPS,
    WEIGHT_DECAY,
    WINDOW_SIZE,
    SimRecipe,
)
from .progress_windows import ProgressWindows, WindowBatch
from .request import ProgressRunRequest
from .volume import TrainingVolume
from .warp_rm_upstream.core.loss import RMLoss

CHECKPOINT_MANIFEST = "spiral_robotics.progress.safetensors.v1"


@dataclass(frozen=True)
class ProgressResources:
    """What the progress job's container provides: the directory holding Score progress's pinned
    DINOv3 ViT-B/16, baked into its image, and how many processes build training batches."""

    backbone: Path
    sampling_workers: int


class ProgressTraining:
    """A progress model's side of a run's job: its camera's embeddings on the job's disk, the
    windows drawn over them, and its session."""

    def __init__(
        self, request: ProgressRunRequest, volume: TrainingVolume, resources: ProgressResources
    ) -> None:
        self._request = request
        self._volume = volume
        self._resources = resources

    @property
    def request(self) -> ProgressRunRequest:
        return self._request

    def preparing(self) -> str:
        camera = self._request.config.model.camera
        return (
            f"Decoding {len(self._request.episodes)} episodes of {camera} and embedding their "
            "pictures with DINOv3 ViT-B/16, as Score progress does."
        )

    def prepare(self, scratch: Path) -> PreparedRun:
        request = self._request
        device = TrainingDevice.available()
        encoder = PictureEncoder(self._resources.backbone, device.torch_device)
        embeddings = CameraEmbeddings(request, self._volume, encoder, scratch)
        camera = embeddings.camera
        windows = ProgressWindows(
            camera, request.config.optimization, self._resources.sampling_workers
        )
        session = ProgressSession(request, device)
        episodes, intervals = len(request.episodes), len(camera.intervals)
        logs = [
            f"Embedded {episodes} episodes of {request.config.model.camera}: {camera.frames} "
            f"pictures at {FPS} Hz in {intervals} kept intervals; skipped {embeddings.skipped} "
            "that hold no picture."
        ]
        short = sum(interval.frames < STANDARD_WINDOW_FRAMES for interval in camera.intervals)
        if short:
            logs.append(
                f"{short} of the {intervals} intervals are shorter than a standard-pace window "
                f"({STANDARD_WINDOW_FRAMES} pictures); the sampler fits its windows into them, as "
                "upstream's does."
            )
        running = (
            f"Training a progress model on {session.device_name}: {episodes} episodes, "
            f"{intervals} intervals, {camera.frames} pictures."
        )
        return PreparedRun(windows, session, logs, running)


class ProgressSession:
    """The progress head and its optimizer on `device`, stepped as upstream's `Trainer.train` steps
    them: the C51 loss, gradients clipped, AdamW, a linear warmup, then a cosine schedule. Unlike
    upstream, the forward pass and the loss run under bf16 autocast on CUDA; the weights and the
    optimizer's state stay fp32."""

    def __init__(self, request: ProgressRunRequest, device: TrainingDevice) -> None:
        self._request = request
        self._device = device
        optimization = request.config.optimization
        torch.manual_seed(optimization.seed)
        # Upstream's stochastic depth drops layers with Python's `random`; its trainer seeds it too.
        random.seed(optimization.seed)
        self._model = SimRecipe.model().to(device.torch_device)
        self._loss = RMLoss(SimRecipe.ablation())
        self._base_rate = optimization.learning_rate
        self._optimizer = torch.optim.AdamW(
            self._model.parameters(), lr=self._base_rate, weight_decay=WEIGHT_DECAY
        )
        self._cosine = torch.optim.lr_scheduler.CosineAnnealingWarmRestarts(
            self._optimizer,
            T_0=max(int(3 * optimization.steps), 1000),
            T_mult=1,
            eta_min=self._base_rate * 0.01,
        )
        self._learning_rate = self._base_rate

    @property
    def device_name(self) -> str:
        return self._device.name

    @property
    def learning_rate(self) -> float:
        return self._learning_rate

    def train_step(self, batch: WindowBatch, step: int) -> StepResult:
        """Step `step`, counted from 1; upstream counts from 0."""
        device_started = time.perf_counter()
        features, labels = self._inputs(batch)
        device_copy_ms = self._device.finished_ms(device_started)
        model_started = time.perf_counter()
        self._model.train()
        self._learning_rate = self._optimizer.param_groups[0]["lr"]
        with self._precision():
            loss = self._loss_of(features, labels)
        if not torch.isfinite(loss):
            raise ValueError(f"the progress loss became non-finite at step {step}")
        self._optimizer.zero_grad()
        loss.backward()
        torch.nn.utils.clip_grad_norm_(self._model.parameters(), GRADIENT_CLIP)
        self._optimizer.step()
        self._schedule_after(step - 1)
        return StepResult(
            float(loss.item()), device_copy_ms, self._device.finished_ms(model_started)
        )

    def validate(self, batch: WindowBatch) -> StepResult:
        with torch.inference_mode():
            device_started = time.perf_counter()
            features, labels = self._inputs(batch)
            device_copy_ms = self._device.finished_ms(device_started)
            model_started = time.perf_counter()
            self._model.eval()
            with self._precision():
                loss = self._loss_of(features, labels)
            return StepResult(
                float(loss.item()), device_copy_ms, self._device.finished_ms(model_started)
            )

    def telemetry(self) -> dict[str, Any]:
        return self._device.telemetry()

    def save_checkpoint(self, directory: Path, step: int) -> None:
        """Writes `model.safetensors`, the head's tensors under upstream `TransformerAggregator`
        names, and `config.json`, the manifest saying how its inputs were made."""
        tensors = {
            name: tensor.detach().to(device="cpu").contiguous()
            for name, tensor in self._model.state_dict().items()
        }
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "config.json").write_text(json.dumps(self._manifest(step), indent=2))
        (directory / "model.safetensors").write_bytes(save_safetensors(tensors))

    def state(self) -> dict[str, Any]:
        return {
            "model": self._model.state_dict(),
            "optimizer": self._optimizer.state_dict(),
            "cosine": self._cosine.state_dict(),
        }

    def restore(self, state: dict[str, Any]) -> None:
        self._model.load_state_dict(state["model"])
        self._optimizer.load_state_dict(state["optimizer"])
        self._cosine.load_state_dict(state["cosine"])

    def _inputs(self, batch: WindowBatch) -> tuple[torch.Tensor, torch.Tensor]:
        device = self._device.torch_device
        return (
            batch.features.to(device, non_blocking=True),
            batch.labels.to(device, non_blocking=True),
        )

    def _precision(self) -> torch.autocast:
        """bf16 autocast on CUDA, which an H100 runs several times faster than fp32; full
        precision elsewhere."""
        device = self._device.torch_device
        return torch.autocast(device.type, dtype=torch.bfloat16, enabled=device.type == "cuda")

    def _loss_of(self, features: torch.Tensor, labels: torch.Tensor) -> torch.Tensor:
        progress, transformed, relative_logits = self._model(features)
        loss, _ = self._loss(
            progress_preds=progress,
            backbone_out=transformed,
            rel_logits=relative_logits,
            model=self._model,
            labels=labels,
            abs_labels=None,
            current_lr=self._learning_rate,
            base_lr=self._base_rate,
        )
        return loss

    def _schedule_after(self, upstream_step: int) -> None:
        """The rate for the next step, as `Trainer.train` sets it after `upstream_step`: a linear
        warmup over the first steps (the very first step trains at the full rate), then the cosine
        schedule, one step at a time."""
        if upstream_step < WARMUP_STEPS:
            for group in self._optimizer.param_groups:
                group["lr"] = self._base_rate * (upstream_step + 1) / WARMUP_STEPS
        else:
            self._cosine.step()

    def _manifest(self, step: int) -> dict[str, Any]:
        model = self._model
        return {
            "family": "progress",
            "manifest": CHECKPOINT_MANIFEST,
            "run_id": self._request.run_id,
            "camera": self._request.config.model.camera,
            "backbone": f"{BACKBONE_REPOSITORY}@{BACKBONE_REVISION}",
            "window_size": WINDOW_SIZE,
            "feature_stride": FEATURE_STRIDE,
            "standard_stride_src": SOURCE_STANDARD_STRIDE,
            "fps": FPS,
            "bins": model.n_rel_bins,
            "bin_range": [model.rel_bin_min, model.rel_bin_max],
            "ablation": ABLATION,
            "image": {"width": IMAGE_SIZE, "height": IMAGE_SIZE},
            "crop": CROP_MODE,
            "step": step,
        }
