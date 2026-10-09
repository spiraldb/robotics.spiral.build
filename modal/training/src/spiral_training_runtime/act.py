"""LeRobot's ACT policy, trained by a run's job, and its checkpoints. See
docs/training.md#checkpoints.
"""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

import draccus
import numpy as np
import torch
from lerobot.configs import FeatureType, PolicyFeature
from lerobot.policies.act import ACTConfig, ACTPolicy
from safetensors.torch import save as save_safetensors

from .device import StepResult, TrainingDevice
from .errors import RuntimeRequestError
from .family import PreparedRun
from .request import ActRunRequest
from .samples import Normalization, TrainingBatch, TrainingSamples
from .volume import TrainingVolume

IMAGENET_MEAN = (0.485, 0.456, 0.406)
IMAGENET_STD = (0.229, 0.224, 0.225)
NORMALIZATION_EPSILON = 1.0e-8
GLOBAL_GRADIENT_NORM_LIMIT = 10.0
WEIGHT_DECAY = 1.0e-4
STATE_FEATURE = "observation.state"
ACTION_FEATURE = "action"
CHECKPOINT_FORMAT = "spiral_robotics.act.safetensors.v2"


class ActTraining:
    """An ACT policy's side of a run's job: its samples on the job's disk and its session."""

    def __init__(self, request: ActRunRequest, volume: TrainingVolume) -> None:
        self._request = request
        self._volume = volume

    @property
    def request(self) -> ActRunRequest:
        return self._request

    def preparing(self) -> str:
        rate_hz = self._request.config.model.observation.rate_hz
        return (
            f"Decoding {len(self._request.episodes)} episodes and sampling them at {rate_hz:g} Hz."
        )

    def prepare(self, scratch: Path) -> PreparedRun:
        request = self._request
        observation = request.config.model.observation
        samples = TrainingSamples(request, self._volume, scratch)
        session = TrainingSession(request, samples.normalization, TrainingDevice.available())
        episodes = len(request.episodes)
        intervals = sum(len(episode.intervals) for episode in request.episodes)
        image = observation.image
        return PreparedRun(
            samples,
            session,
            logs=[
                f"Prepared {episodes} episodes: {samples.ticks} ticks of "
                f"{len(observation.cameras)} cameras at {image.width}x{image.height}."
            ],
            running=f"Training ACT on {session.device_name}: {episodes} episodes, "
            f"{intervals} intervals, {samples.ticks} ticks.",
        )


class TrainingSession:
    """One run's ACT policy and optimizer, trained on normalized batches on `device`, and saved as
    inference checkpoints with the normalization the samples were taken with."""

    def __init__(
        self, request: ActRunRequest, normalization: Normalization, device: TrainingDevice
    ) -> None:
        self._request = request
        self._normalization = normalization
        self._device = device
        optimization = request.config.optimization
        torch.manual_seed(optimization.seed)
        self._policy = ACTPolicy(self._policy_config()).to(device.torch_device)
        self._optimizer = torch.optim.AdamW(
            self._policy.get_optim_params(),
            lr=optimization.learning_rate,
            weight_decay=WEIGHT_DECAY,
        )
        self._image_normalization = (
            torch.tensor(IMAGENET_MEAN, device=device.torch_device).view(1, 1, 3, 1, 1),
            torch.tensor(IMAGENET_STD, device=device.torch_device).view(1, 1, 3, 1, 1),
        )

    @property
    def device_name(self) -> str:
        return self._device.name

    @property
    def learning_rate(self) -> float:
        return self._request.config.optimization.learning_rate

    def train_step(self, batch: TrainingBatch, step: int) -> StepResult:
        device_started = time.perf_counter()
        inputs = self._inputs(batch)
        device_copy_ms = self._device.finished_ms(device_started)
        model_started = time.perf_counter()
        self._policy.train()
        self._optimizer.zero_grad(set_to_none=True)
        loss, _ = self._policy(inputs)
        if not torch.isfinite(loss):
            raise RuntimeRequestError(f"ACT loss became non-finite at step {step}", 422)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(self._policy.parameters(), GLOBAL_GRADIENT_NORM_LIMIT)
        self._optimizer.step()
        return StepResult(
            float(loss.item()), device_copy_ms, self._device.finished_ms(model_started)
        )

    def validate(self, batch: TrainingBatch) -> StepResult:
        with torch.inference_mode():
            device_started = time.perf_counter()
            inputs = self._inputs(batch)
            device_copy_ms = self._device.finished_ms(device_started)
            model_started = time.perf_counter()
            self._policy.eval()
            loss, _ = self._policy(inputs)
            return StepResult(
                float(loss.item()), device_copy_ms, self._device.finished_ms(model_started)
            )

    def telemetry(self) -> dict[str, Any]:
        return self._device.telemetry()

    def save_checkpoint(self, directory: Path, step: int) -> None:
        """Writes `config.json` and `model.safetensors`: the policy's tensors with the
        normalization buffers, and the manifest in the SafeTensors metadata."""
        normalization = self._normalization
        cameras = self._request.camera_features()
        tensors = {
            name: tensor.detach().to(device="cpu").contiguous()
            for name, tensor in self._policy.state_dict().items()
        }
        buffers = {
            ("normalize_inputs", STATE_FEATURE, "mean"): normalization.state_mean,
            ("normalize_inputs", STATE_FEATURE, "std"): normalization.state_std,
            ("unnormalize_outputs", ACTION_FEATURE, "mean"): normalization.action_mean,
            ("unnormalize_outputs", ACTION_FEATURE, "std"): normalization.action_std,
        }
        for camera in cameras:
            buffers[("normalize_inputs", camera, "mean")] = np.reshape(IMAGENET_MEAN, (3, 1, 1))
            buffers[("normalize_inputs", camera, "std")] = np.reshape(IMAGENET_STD, (3, 1, 1))
        for (operation, feature, statistic), values in buffers.items():
            name = _normalization_name(operation, feature, statistic)
            tensors[name] = torch.tensor(values, dtype=torch.float32)
        observation = self._request.config.model.observation
        config = json.loads(self._dump_policy_config())
        config["spiral_camera_order"] = cameras
        config["spiral_sample_hz"] = observation.rate_hz
        manifest = {
            "format": CHECKPOINT_FORMAT,
            "run_id": self._request.run_id,
            "step": step,
            "data_contract": {
                "robot_type": self._request.robot_type,
                "joint_names": self._request.joints.state,
                "camera_streams": observation.cameras,
                "sample_hz": observation.rate_hz,
                "image_size": observation.image.model_dump(),
            },
        }
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "config.json").write_text(json.dumps(config, indent=2))
        weights = save_safetensors(tensors, metadata={"spiral_robotics": json.dumps(manifest)})
        (directory / "model.safetensors").write_bytes(weights)

    def state(self) -> dict[str, Any]:
        """The model and optimizer, for a retried job to resume from."""
        return {"model": self._policy.state_dict(), "optimizer": self._optimizer.state_dict()}

    def restore(self, state: dict[str, Any]) -> None:
        self._policy.load_state_dict(state["model"])
        self._optimizer.load_state_dict(state["optimizer"])

    def _policy_config(self) -> ACTConfig:
        request = self._request
        image = request.config.model.observation.image
        act = request.config.model.architecture
        learning_rate = request.config.optimization.learning_rate
        chunk_size = request.config.model.chunk_size
        return ACTConfig(
            input_features=_input_features(
                len(request.joints.state),
                tuple(request.camera_features()),
                image.height,
                image.width,
            ),
            output_features=_output_features(len(request.joints.action)),
            device=str(self._device.torch_device),
            chunk_size=chunk_size,
            n_action_steps=chunk_size,
            vision_backbone="resnet18",
            pretrained_backbone_weights=(
                "ResNet18_Weights.IMAGENET1K_V1" if act.pretrained_backbone else None
            ),
            pre_norm=False,
            dim_model=act.hidden_dim,
            n_heads=act.attention_heads,
            dim_feedforward=act.feedforward_dim,
            feedforward_activation="relu",
            n_encoder_layers=act.encoder_layers,
            n_decoder_layers=act.decoder_layers,
            use_vae=True,
            latent_dim=act.latent_dim,
            n_vae_encoder_layers=act.vae_encoder_layers,
            dropout=0.1,
            kl_weight=act.kl_weight,
            optimizer_lr=learning_rate,
            optimizer_lr_backbone=learning_rate,
            optimizer_weight_decay=WEIGHT_DECAY,
        )

    def _dump_policy_config(self) -> str:
        """LeRobot's own JSON for the policy config, as its `save_pretrained` writes it."""
        with draccus.config_type("json"):
            return draccus.dump(self._policy.config)

    def _inputs(self, batch: TrainingBatch) -> dict[str, torch.Tensor]:
        device = self._device.torch_device
        images = torch.from_numpy(batch.images).to(device)
        images = _normalize_hwc_images(images, *self._image_normalization)
        inputs = {
            STATE_FEATURE: torch.from_numpy(batch.states).to(device),
            ACTION_FEATURE: torch.from_numpy(batch.actions).to(device),
            "action_is_pad": torch.from_numpy(batch.action_is_pad).to(device),
        }
        for index, camera in enumerate(self._request.camera_features()):
            inputs[camera] = images[:, index]
        return inputs


def _input_features(
    state_dimensions: int,
    cameras: tuple[str, ...],
    height: int,
    width: int,
) -> dict[str, PolicyFeature]:
    features = {STATE_FEATURE: PolicyFeature(type=FeatureType.STATE, shape=(state_dimensions,))}
    features.update(
        {
            camera: PolicyFeature(type=FeatureType.VISUAL, shape=(3, height, width))
            for camera in cameras
        }
    )
    return features


def _output_features(action_dimensions: int) -> dict[str, PolicyFeature]:
    return {ACTION_FEATURE: PolicyFeature(type=FeatureType.ACTION, shape=(action_dimensions,))}


def _normalize_hwc_images(
    images: torch.Tensor,
    mean: torch.Tensor,
    std: torch.Tensor,
) -> torch.Tensor:
    """Convert device-resident HWC bytes into normalized CHW model inputs."""
    images = images.permute(0, 1, 4, 2, 3).to(dtype=torch.float32).div_(255.0)
    return (images - mean) / (std + NORMALIZATION_EPSILON)


def _normalization_name(operation: str, feature: str, statistic: str) -> str:
    safe_feature = feature.replace(".", "_").replace("/", "_")
    return f"{operation}.buffer_{safe_feature}.{statistic}"
