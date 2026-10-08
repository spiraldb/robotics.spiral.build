"""LeRobot's ACT policy: trained by a run's job, and served for rollout from its checkpoints.

Training and inference share this one implementation, so a checkpoint the job writes loads in
`PolicySession.from_artifact`. See docs/training.md#checkpoints.
"""

from __future__ import annotations

import json
import math
import threading
import time
import warnings
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import draccus
import numpy as np
import torch
from lerobot.configs import FeatureType, PolicyFeature
from lerobot.policies.act import ACTConfig, ACTPolicy
from safetensors.torch import load as load_safetensors
from safetensors.torch import save as save_safetensors

from .arrow import InferenceArrays
from .config import PolicyConfig
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


def resolve_device(requested: str) -> torch.device:
    value = requested.strip().lower()
    if value == "auto":
        if torch.cuda.is_available():
            return torch.device("cuda:0")
        if torch.backends.mps.is_available():
            return torch.device("mps")
        return torch.device("cpu")
    try:
        device = torch.device(value)
    except RuntimeError as error:
        raise RuntimeRequestError(f"invalid PyTorch device {requested!r}: {error}", 422) from error
    if device.type == "cuda":
        if not torch.cuda.is_available():
            raise RuntimeRequestError("CUDA was requested but is unavailable", 422)
        index = device.index or 0
        if index >= torch.cuda.device_count():
            raise RuntimeRequestError(f"CUDA device {index} does not exist", 422)
        return torch.device("cuda", index)
    if device.type == "mps" and not torch.backends.mps.is_available():
        raise RuntimeRequestError("MPS was requested but is unavailable", 422)
    if device.type not in {"cpu", "cuda", "mps"}:
        raise RuntimeRequestError(
            f"PyTorch device {requested!r} is unsupported; use auto, cpu, cuda, or mps", 422
        )
    return device


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


@dataclass
class PolicySession:
    config: PolicyConfig
    device: torch.device
    policy: ACTPolicy
    state_mean: torch.Tensor
    state_std: torch.Tensor
    image_mean: tuple[torch.Tensor, ...]
    image_std: tuple[torch.Tensor, ...]
    action_mean: torch.Tensor
    action_std: torch.Tensor
    lock: threading.Lock

    @classmethod
    def from_artifact(
        cls, config_bytes: bytes, weights_bytes: bytes, requested_device: str
    ) -> PolicySession:
        try:
            raw_config = json.loads(config_bytes)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise RuntimeRequestError(f"invalid ACT policy config: {error}", 422) from error
        if not isinstance(raw_config, dict):
            raise RuntimeRequestError("ACT policy config must be a JSON object", 422)
        config = PolicyConfig.from_json(raw_config)
        device = resolve_device(requested_device)
        policy = ACTPolicy(_inference_policy_config(config, device)).to(device)
        try:
            checkpoint = load_safetensors(weights_bytes)
        except Exception as error:
            raise RuntimeRequestError(
                f"invalid SafeTensors policy checkpoint: {error}", 422
            ) from error
        model_state = {
            name: tensor for name, tensor in checkpoint.items() if name.startswith("model.")
        }
        try:
            policy.load_state_dict(model_state, strict=True)
        except RuntimeError as error:
            raise RuntimeRequestError(
                f"ACT checkpoint does not match its config: {error}", 422
            ) from error
        policy.eval()
        state_mean = _checkpoint_tensor(
            checkpoint,
            _normalization_name("normalize_inputs", config.state_name, "mean"),
            (config.state_dimensions,),
            device,
        )
        state_std = _checkpoint_tensor(
            checkpoint,
            _normalization_name("normalize_inputs", config.state_name, "std"),
            (config.state_dimensions,),
            device,
        )
        image_mean = tuple(
            _checkpoint_tensor(
                checkpoint,
                _normalization_name("normalize_inputs", camera, "mean"),
                (3, 1, 1),
                device,
            )
            for camera in config.camera_streams
        )
        image_std = tuple(
            _checkpoint_tensor(
                checkpoint,
                _normalization_name("normalize_inputs", camera, "std"),
                (3, 1, 1),
                device,
            )
            for camera in config.camera_streams
        )
        action_mean = _checkpoint_tensor(
            checkpoint,
            _normalization_name("unnormalize_outputs", config.action_name, "mean"),
            (config.action_dimensions,),
            device,
        )
        action_std = _checkpoint_tensor(
            checkpoint,
            _normalization_name("unnormalize_outputs", config.action_name, "std"),
            (config.action_dimensions,),
            device,
        )
        return cls(
            config=config,
            device=device,
            policy=policy,
            state_mean=state_mean,
            state_std=state_std,
            image_mean=image_mean,
            image_std=image_std,
            action_mean=action_mean,
            action_std=action_std,
            lock=threading.Lock(),
        )

    @property
    def specification(self) -> dict[str, Any]:
        return {
            "kind": "act",
            "state_dimensions": self.config.state_dimensions,
            "image_inputs": [
                {
                    "name": name,
                    "channels": 3,
                    "height": self.config.image_height,
                    "width": self.config.image_width,
                }
                for name in self.config.camera_streams
            ],
            "action_dimensions": self.config.action_dimensions,
            "action_chunk_size": self.config.chunk_size,
            "recommended_action_steps": self.config.action_steps,
        }

    def warm_up(self) -> dict[str, float]:
        state = np.zeros((1, self.config.state_dimensions), dtype=np.float32)
        images = tuple(
            np.zeros((1, 3 * self.config.image_height * self.config.image_width), dtype=np.uint8)
            for _ in self.config.camera_streams
        )
        # No Arrow owner is needed for runtime-owned warm-up arrays.
        return self.infer(InferenceArrays(owner=None, state=state, images=images))["stats"]

    def infer(self, arrays: InferenceArrays) -> dict[str, Any]:
        with self.lock, torch.inference_mode():
            started = time.perf_counter()
            state = _from_numpy(arrays.state, self.device)
            state = (state - self.state_mean) / (self.state_std + NORMALIZATION_EPSILON)
            batch: dict[str, torch.Tensor] = {STATE_FEATURE: state}
            for index, camera in enumerate(self.config.camera_streams):
                image = _from_numpy(arrays.images[index], self.device).reshape(
                    1, self.config.image_height, self.config.image_width, 3
                )
                image = image.permute(0, 3, 1, 2).to(dtype=torch.float32).div_(255.0)
                image = (image - self.image_mean[index]) / (
                    self.image_std[index] + NORMALIZATION_EPSILON
                )
                batch[camera] = image
            _synchronize(self.device)
            preprocessing_ms = _elapsed_ms(started)

            model_started = time.perf_counter()
            actions = self.policy.predict_action_chunk(batch)
            _synchronize(self.device)
            model_ms = _elapsed_ms(model_started)

            post_started = time.perf_counter()
            actions = actions * self.action_std + self.action_mean
            values = actions.to(device="cpu", dtype=torch.float32).reshape(-1).tolist()
            if any(not math.isfinite(value) for value in values):
                raise RuntimeRequestError("ACT policy produced a non-finite action", 422)
            postprocessing_ms = _elapsed_ms(post_started)
            return {
                "values": values,
                "steps": self.config.chunk_size,
                "action_dimensions": self.config.action_dimensions,
                "stats": {
                    "preprocessing_ms": preprocessing_ms,
                    "model_ms": model_ms,
                    "postprocessing_ms": postprocessing_ms,
                    "total_ms": _elapsed_ms(started),
                },
            }


def _inference_policy_config(config: PolicyConfig, device: torch.device) -> ACTConfig:
    return ACTConfig(
        input_features=_input_features(
            config.state_dimensions,
            config.camera_streams,
            config.image_height,
            config.image_width,
        ),
        output_features=_output_features(config.action_dimensions),
        device=str(device),
        chunk_size=config.chunk_size,
        n_action_steps=config.action_steps,
        vision_backbone="resnet18",
        pretrained_backbone_weights=None,
        pre_norm=config.pre_norm,
        dim_model=config.hidden_dimensions,
        n_heads=config.attention_heads,
        dim_feedforward=config.feedforward_dimensions,
        feedforward_activation="relu",
        n_encoder_layers=config.encoder_layers,
        n_decoder_layers=config.decoder_layers,
        use_vae=config.use_vae,
        latent_dim=config.latent_dimensions,
        n_vae_encoder_layers=config.vae_encoder_layers,
        dropout=config.dropout,
    )


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


def _from_numpy(array: np.ndarray, device: torch.device) -> torch.Tensor:
    # Arrow owns this read-only CPU buffer for the complete synchronous call.
    # Model inputs are never mutated, so sharing it is safe despite PyTorch's
    # generic warning for non-writable NumPy arrays.
    with warnings.catch_warnings():
        warnings.filterwarnings(
            "ignore",
            message="The given NumPy array is not writable",
            category=UserWarning,
        )
        tensor = torch.from_numpy(array)
    if device.type == "cpu":
        return tensor
    return tensor.to(device=device, non_blocking=False)


def _normalize_hwc_images(
    images: torch.Tensor,
    mean: torch.Tensor,
    std: torch.Tensor,
) -> torch.Tensor:
    """Convert device-resident HWC bytes into normalized CHW model inputs."""
    images = images.permute(0, 1, 4, 2, 3).to(dtype=torch.float32).div_(255.0)
    return (images - mean) / (std + NORMALIZATION_EPSILON)


def _checkpoint_tensor(
    checkpoint: dict[str, torch.Tensor],
    name: str,
    shape: tuple[int, ...],
    device: torch.device,
) -> torch.Tensor:
    tensor = checkpoint.get(name)
    if tensor is None or tensor.dtype != torch.float32 or tuple(tensor.shape) != shape:
        actual = None if tensor is None else (str(tensor.dtype), tuple(tensor.shape))
        raise RuntimeRequestError(
            f"checkpoint tensor {name!r} must be float32 {shape}, found {actual}", 422
        )
    if not torch.isfinite(tensor).all():
        raise RuntimeRequestError(f"checkpoint tensor {name!r} contains non-finite values", 422)
    return tensor.to(device=device)


def _normalization_name(operation: str, feature: str, statistic: str) -> str:
    safe_feature = feature.replace(".", "_").replace("/", "_")
    return f"{operation}.buffer_{safe_feature}.{statistic}"


def _synchronize(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.synchronize(device)
    elif device.type == "mps":
        torch.mps.synchronize()


def _elapsed_ms(started: float) -> float:
    return (time.perf_counter() - started) * 1_000.0
