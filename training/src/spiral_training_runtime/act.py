from __future__ import annotations

import json
import math
import threading
import time
import warnings
from dataclasses import dataclass
from importlib.metadata import version
from typing import Any

import numpy as np
import psutil
import torch
from lerobot.configs import FeatureType, PolicyFeature
from lerobot.policies.act import ACTConfig, ACTPolicy
from safetensors.torch import load as load_safetensors
from safetensors.torch import save as save_safetensors

from .arrow import InferenceArrays, TrainingArrays
from .config import PolicyConfig, TrainingConfig
from .errors import RuntimeRequestError
from .protocol import BOUNDARIES

IMAGENET_MEAN = (0.485, 0.456, 0.406)
IMAGENET_STD = (0.229, 0.224, 0.225)
NORMALIZATION_EPSILON = 1.0e-8
GLOBAL_GRADIENT_NORM_LIMIT = 10.0
BUFFER_WAIT_SECONDS = 30.0


def backend_info() -> dict[str, Any]:
    devices = ["CPU"]
    if torch.cuda.is_available():
        devices = [
            f"CUDA:{index} {torch.cuda.get_device_name(index)}"
            for index in range(torch.cuda.device_count())
        ] + devices
    if torch.backends.mps.is_available():
        devices.insert(0, "MPS")
    return {
        "api_id": BOUNDARIES.training_api.id,
        "api_version": BOUNDARIES.training_api.major,
        "api_revision": BOUNDARIES.training_api.minor,
        "spiral_schema_minimum": BOUNDARIES.training_api.spiral_schema.minimum,
        "spiral_schema_maximum": BOUNDARIES.training_api.spiral_schema.maximum,
        "id": "pytorch.lerobot",
        "display_name": "PyTorch ACT",
        "runtime": f"PyTorch {torch.__version__} / LeRobot {version('lerobot')}",
        "devices": devices,
    }


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


class TrainingSession:
    def __init__(self, config: TrainingConfig) -> None:
        self.config = config
        self.device = resolve_device(config.requested_device)
        torch.manual_seed(config.seed)
        if torch.cuda.is_available():
            torch.cuda.manual_seed_all(config.seed)
        policy_config = _training_policy_config(config, self.device)
        self.policy = ACTPolicy(policy_config).to(self.device)
        self.optimizer = torch.optim.AdamW(
            self.policy.get_optim_params(),
            lr=config.learning_rate,
            weight_decay=1.0e-4,
        )
        self.parameter_count = sum(
            parameter.numel() for parameter in self.policy.parameters() if parameter.requires_grad
        )
        self.lock = threading.Lock()
        self.last_step: int | None = None
        self.last_response: dict[str, Any] | None = None
        self.buffer_condition = threading.Condition()
        self.buffered_batch: tuple[str, int, TrainingArrays] | None = None
        self.process = psutil.Process()
        self.process.cpu_percent(interval=None)
        self.image_mean = torch.tensor(IMAGENET_MEAN, dtype=torch.float32, device=self.device).view(
            1, 1, 3, 1, 1
        )
        self.image_std = torch.tensor(IMAGENET_STD, dtype=torch.float32, device=self.device).view(
            1, 1, 3, 1, 1
        )

    @property
    def info(self) -> dict[str, Any]:
        return {
            "backend": backend_info(),
            "device": _device_name(self.device),
            "parameter_count": self.parameter_count,
        }

    def buffer_batch(self, arrays: TrainingArrays, kind: str, step: int) -> None:
        deadline = time.monotonic() + BUFFER_WAIT_SECONDS
        with self.buffer_condition:
            while self.buffered_batch is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeRequestError(
                        "training session upload buffer remained occupied", 409
                    )
                self.buffer_condition.wait(remaining)
            self.buffered_batch = (kind, step, arrays)

    def _take_buffered_batch(self, kind: str, step: int) -> TrainingArrays:
        with self.buffer_condition:
            if self.buffered_batch is None:
                raise RuntimeRequestError(f"{kind} step {step} has no buffered Arrow batch", 409)
            buffered_kind, buffered_step, arrays = self.buffered_batch
            if buffered_kind != kind or buffered_step != step:
                raise RuntimeRequestError(
                    f"{kind} step {step} cannot consume buffered "
                    f"{buffered_kind} step {buffered_step}",
                    409,
                )
            self.buffered_batch = None
            self.buffer_condition.notify_all()
            return arrays

    def train_buffered(self, step: int) -> dict[str, Any]:
        arrays = self._take_buffered_batch("training", step)
        with self.lock:
            if self.last_step == step and self.last_response is not None:
                return self.last_response
            if self.last_step is not None and step != self.last_step + 1:
                raise RuntimeRequestError(
                    f"optimizer step {step} cannot follow completed step {self.last_step}", 409
                )
            if step <= 0 or step > self.config.total_steps:
                raise RuntimeRequestError(
                    f"optimizer step must be between 1 and {self.config.total_steps}", 409
                )
            device_started = time.perf_counter()
            batch = self._batch(arrays)
            _synchronize(self.device)
            device_copy_ms = _elapsed_ms(device_started)
            model_started = time.perf_counter()
            self.policy.train()
            self.optimizer.zero_grad(set_to_none=True)
            loss, loss_parts = self.policy(batch)
            if not torch.isfinite(loss):
                raise RuntimeRequestError(f"ACT loss became non-finite at step {step}", 422)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(self.policy.parameters(), GLOBAL_GRADIENT_NORM_LIMIT)
            self.optimizer.step()
            _synchronize(self.device)
            model_ms = _elapsed_ms(model_started)
            response = self._step_response(loss, loss_parts, device_copy_ms, model_ms)
            self.last_step = step
            self.last_response = response
            return response

    def validate_buffered(self, after_step: int) -> dict[str, Any]:
        arrays = self._take_buffered_batch("validation", after_step)
        with self.lock, torch.inference_mode():
            if self.last_step != after_step:
                raise RuntimeRequestError(
                    f"validation after step {after_step} cannot follow completed step "
                    f"{self.last_step}",
                    409,
                )
            device_started = time.perf_counter()
            batch = self._batch(arrays)
            _synchronize(self.device)
            device_copy_ms = _elapsed_ms(device_started)
            model_started = time.perf_counter()
            self.policy.eval()
            loss, loss_parts = self.policy(batch)
            _synchronize(self.device)
            model_ms = _elapsed_ms(model_started)
            return self._step_response(loss, loss_parts, device_copy_ms, model_ms)

    def telemetry(self) -> dict[str, Any]:
        return {
            "cpu_percent": float(self.process.cpu_percent(interval=None)),
            "gpu_percent": _gpu_utilization(self.device),
            "memory_bytes": int(self.process.memory_info().rss),
        }

    def checkpoint(self, request: dict[str, Any]) -> bytes:
        with self.lock:
            state_mean = _normalization_vector(request, "state_mean", self.config.state_dimensions)
            state_std = _normalization_vector(request, "state_std", self.config.state_dimensions)
            action_mean = _normalization_vector(
                request, "action_mean", self.config.action_dimensions
            )
            action_std = _normalization_vector(request, "action_std", self.config.action_dimensions)
            manifest = request.get("manifest_json")
            if not isinstance(manifest, str):
                raise RuntimeRequestError("checkpoint manifest_json must be a string", 422)
            tensors = {
                name: tensor.detach().to(device="cpu").contiguous()
                for name, tensor in self.policy.state_dict().items()
            }
            tensors[_normalization_name("normalize_inputs", "observation.state", "mean")] = (
                state_mean
            )
            tensors[_normalization_name("normalize_inputs", "observation.state", "std")] = state_std
            for camera in self.config.camera_streams:
                tensors[_normalization_name("normalize_inputs", camera, "mean")] = torch.tensor(
                    IMAGENET_MEAN, dtype=torch.float32
                ).view(3, 1, 1)
                tensors[_normalization_name("normalize_inputs", camera, "std")] = torch.tensor(
                    IMAGENET_STD, dtype=torch.float32
                ).view(3, 1, 1)
            tensors[_normalization_name("unnormalize_outputs", "action", "mean")] = action_mean
            tensors[_normalization_name("unnormalize_outputs", "action", "std")] = action_std
            return save_safetensors(
                tensors,
                metadata={"format": "spiral-act", "spiral_robotics": manifest},
            )

    def _batch(self, arrays: TrainingArrays) -> dict[str, torch.Tensor]:
        batch_size = arrays.batch_size
        states = _from_numpy(arrays.states, self.device).reshape(
            batch_size, self.config.state_dimensions
        )
        images = _from_numpy(arrays.images, self.device).reshape(
            batch_size,
            len(self.config.camera_streams),
            self.config.image_height,
            self.config.image_width,
            3,
        )
        images = _normalize_hwc_images(images, self.image_mean, self.image_std)
        actions = _from_numpy(arrays.actions, self.device).reshape(
            batch_size, self.config.chunk_size, self.config.action_dimensions
        )
        padding = _from_numpy(arrays.action_is_pad, self.device).reshape(
            batch_size, self.config.chunk_size
        )
        batch: dict[str, torch.Tensor] = {
            "observation.state": states,
            "action": actions,
            "action_is_pad": padding.bool(),
        }
        for index, camera in enumerate(self.config.camera_streams):
            batch[camera] = images[:, index]
        return batch

    def _step_response(
        self,
        loss: torch.Tensor,
        loss_parts: dict[str, float],
        device_copy_ms: float,
        model_ms: float,
    ) -> dict[str, Any]:
        l1 = float(loss_parts.get("l1_loss", loss.detach().item()))
        kl = float(loss_parts.get("kld_loss", 0.0))
        telemetry = self.telemetry()
        telemetry.update(
            {
                "device_copy_ms": device_copy_ms,
                "model_ms": model_ms,
            }
        )
        return {
            "loss": {
                "total": float(loss.detach().item()),
                "l1": l1,
                "kl": kl,
                "pad": 0.0,
            },
            "learning_rate": self.config.learning_rate,
            "telemetry": telemetry,
        }


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
            batch: dict[str, torch.Tensor] = {"observation.state": state}
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


def _training_policy_config(config: TrainingConfig, device: torch.device) -> ACTConfig:
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
        n_action_steps=config.chunk_size,
        vision_backbone="resnet18",
        pretrained_backbone_weights=(
            "ResNet18_Weights.IMAGENET1K_V1" if config.pretrained_backbone else None
        ),
        pre_norm=False,
        dim_model=config.hidden_dimensions,
        n_heads=config.attention_heads,
        dim_feedforward=config.feedforward_dimensions,
        feedforward_activation="relu",
        n_encoder_layers=config.encoder_layers,
        n_decoder_layers=config.decoder_layers,
        use_vae=True,
        latent_dim=config.latent_dimensions,
        n_vae_encoder_layers=config.vae_encoder_layers,
        dropout=0.1,
        kl_weight=config.kl_weight,
        optimizer_lr=config.learning_rate,
        optimizer_lr_backbone=config.learning_rate,
        optimizer_weight_decay=1.0e-4,
    )


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
    features = {
        "observation.state": PolicyFeature(type=FeatureType.STATE, shape=(state_dimensions,))
    }
    features.update(
        {
            camera: PolicyFeature(type=FeatureType.VISUAL, shape=(3, height, width))
            for camera in cameras
        }
    )
    return features


def _output_features(action_dimensions: int) -> dict[str, PolicyFeature]:
    return {"action": PolicyFeature(type=FeatureType.ACTION, shape=(action_dimensions,))}


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


def _normalization_vector(request: dict[str, Any], name: str, expected: int) -> torch.Tensor:
    values = request.get(name)
    if not isinstance(values, list) or len(values) != expected:
        raise RuntimeRequestError(f"checkpoint {name} must contain {expected} values", 422)
    try:
        tensor = torch.tensor(values, dtype=torch.float32)
    except (TypeError, ValueError) as error:
        raise RuntimeRequestError(f"checkpoint {name} contains an invalid value", 422) from error
    if not torch.isfinite(tensor).all():
        raise RuntimeRequestError(f"checkpoint {name} must contain only finite values", 422)
    return tensor


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


def _device_name(device: torch.device) -> str:
    if device.type == "cuda":
        return f"CUDA:{device.index or 0}"
    return device.type.upper()


def _gpu_utilization(device: torch.device) -> float | None:
    if device.type != "cuda":
        return None
    try:
        return float(torch.cuda.utilization(device))
    except (AttributeError, ImportError, RuntimeError, ValueError):
        return None


def _synchronize(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.synchronize(device)
    elif device.type == "mps":
        torch.mps.synchronize()


def _elapsed_ms(started: float) -> float:
    return (time.perf_counter() - started) * 1_000.0
