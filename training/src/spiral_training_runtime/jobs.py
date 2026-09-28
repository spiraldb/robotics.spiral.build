from __future__ import annotations

import hashlib
import json
import math
import threading
import time
from collections import deque
from typing import Any

from .act import TrainingSession, backend_info
from .config import TrainingConfig
from .errors import RuntimeRequestError
from .mcap import McapTrainingDataset

MAX_ROLLOUT_ACTION_STEPS = 100
MAX_STEP_METRICS = 3_584
MAX_EVENT_METRICS = 512
MAX_LOG_HISTORY = 512


class MetricSeries:
    """A whole run's metrics in bounded memory.

    When the series fills, every other point is dropped and later points are
    kept half as often, so the retained points always span the whole run.
    """

    def __init__(self, capacity: int) -> None:
        self.capacity = capacity
        self.points: list[dict[str, Any]] = []
        self.stride = 1
        self.offered = 0

    def append(self, point: dict[str, Any]) -> None:
        if self.offered % self.stride == 0:
            self.points.append(point)
        self.offered += 1
        if len(self.points) > self.capacity:
            self.points = self.points[::2]
            self.stride *= 2


class TrainingJob:
    """A worker-owned data/model/checkpoint lifecycle and its retained history."""

    def __init__(self, request: dict[str, Any]) -> None:
        self.job_id = self.request_id(request)
        self.request_digest = self._request_digest(request)
        self.lock = threading.Lock()
        self.stop_event = threading.Event()
        self.phase = "starting"
        self.current_step = 0
        self.message: str | None = "Opening SpiralDB and resolving the ACT data contract…"
        self.step_metrics = MetricSeries(MAX_STEP_METRICS)
        # Validation points are rarer and each one matters, so they are kept apart.
        self.event_metrics = MetricSeries(MAX_EVENT_METRICS)
        self.logs: deque[str] = deque(maxlen=MAX_LOG_HISTORY)
        self.thread = threading.Thread(
            target=self._run, args=(request,), name=f"training-{self.job_id}", daemon=True
        )

    def start(self) -> None:
        self.thread.start()

    @staticmethod
    def request_id(request: dict[str, Any]) -> str:
        job_id = request.get("job_id")
        if not isinstance(job_id, str) or not job_id.strip():
            raise RuntimeRequestError("training job_id must be non-empty", 422)
        return job_id

    @staticmethod
    def _request_digest(request: dict[str, Any]) -> bytes:
        canonical = json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
        return hashlib.sha256(canonical).digest()

    def matches(self, request: dict[str, Any]) -> bool:
        return self._request_digest(request) == self.request_digest

    def stop(self) -> None:
        self.stop_event.set()
        with self.lock:
            if self.phase in {"starting", "running"}:
                self.phase = "stopping"
                self.message = "Finishing the current Spiral read or optimizer step…"

    def terminal(self) -> bool:
        with self.lock:
            return self.phase in {"completed", "stopped", "failed"}

    def state(self) -> dict[str, Any]:
        with self.lock:
            return self._state_locked()

    def history(self) -> dict[str, Any]:
        with self.lock:
            metrics = sorted(
                self.step_metrics.points + self.event_metrics.points,
                key=lambda metric: metric["step"],
            )
            return {
                **self._state_locked(),
                "metrics": [dict(metric) for metric in metrics],
                "logs": list(self.logs),
            }

    def _state_locked(self) -> dict[str, Any]:
        return {
            "job_id": self.job_id,
            "phase": self.phase,
            "current_step": self.current_step,
            "message": self.message,
        }

    def _append_metric_locked(self, metric: dict[str, Any]) -> None:
        if metric["event"] is None:
            self.step_metrics.append(metric)
        else:
            self.event_metrics.append(metric)

    def _run(self, request: dict[str, Any]) -> None:
        config = request["config"]
        dataset = None
        try:
            dataset = McapTrainingDataset()
            model = TrainingConfig.from_request(_session_request(config, dataset))
            session = TrainingSession(model)
            with self.lock:
                self.phase = "running"
                self.message = (
                    f"Vision ACT is running on {session.device_name}: {dataset.summary}; "
                    f"{session.parameter_count / 1_000_000.0:.1f}M parameters."
                )
                self.logs.append(
                    "The worker owns SpiralDB reads, SPQL image decode, model execution, "
                    "and checkpoint writes; Studio carries no training batches."
                )
            validation_every = min(250, max(1, math.ceil(int(config["checkpoint_every"]) / 4)))
            started = time.perf_counter()
            last_validation: float | None = None
            last_checkpoint = 0
            for step in range(1, int(config["steps"]) + 1):
                if self.stop_event.is_set():
                    break
                arrays, loader_ms = dataset.fetch(False)
                step_started = time.perf_counter()
                response = session.train_step(arrays, step)
                validation_loss = None
                if dataset.has_validation and step % validation_every == 0:
                    validation, validation_loader_ms = dataset.fetch(True)
                    loader_ms += validation_loader_ms
                    validation_loss = session.validate(validation)["loss"]
                    last_validation = validation_loss
                elapsed = time.perf_counter() - started
                step_seconds = max(time.perf_counter() - step_started, 1.0e-9)
                telemetry = response["telemetry"]
                metric = {
                    "step": step,
                    "train_loss": response["loss"],
                    "validation_loss": validation_loss,
                    "learning_rate": float(response["learning_rate"]),
                    "elapsed_seconds": elapsed,
                    "event": "validation" if validation_loss is not None else None,
                    "examples_per_second": int(config["batch_size"]) / step_seconds,
                    "cpu_percent": telemetry.get("cpu_percent"),
                    "gpu_percent": telemetry.get("gpu_percent"),
                    "memory_bytes": telemetry.get("memory_bytes"),
                    "loader_ms": loader_ms,
                    "device_copy_ms": telemetry.get("device_copy_ms"),
                    "model_ms": telemetry.get("model_ms"),
                }
                with self.lock:
                    self.current_step = step
                    self._append_metric_locked(metric)
                if step % int(config["checkpoint_every"]) == 0:
                    self._checkpoint(dataset, session, request, step, last_validation)
                    last_checkpoint = step
            completed = self.current_step
            if completed > 0 and last_checkpoint != completed:
                self._checkpoint(dataset, session, request, completed, last_validation)
            with self.lock:
                stopped = self.stop_event.is_set()
                self.phase = "stopped" if stopped else "completed"
                self.message = (
                    f"Stopped after {completed} optimizer step(s)."
                    if stopped
                    else f"Completed {completed} optimizer step(s)."
                )
        except Exception as error:
            with self.lock:
                self.phase = "failed"
                self.message = str(error)
                self.logs.append(f"Training failed: {error}")
        finally:
            if dataset is not None:
                dataset.close()

    def _checkpoint(
        self,
        dataset: McapTrainingDataset,
        session: TrainingSession,
        request: dict[str, Any],
        step: int,
        validation_loss: float | None,
    ) -> None:
        manifest, model_config = _checkpoint_manifest(request, dataset, session, step)
        payload = session.checkpoint(
            {
                "manifest_json": json.dumps(manifest, separators=(",", ":")),
                "state_mean": dataset.state_mean.tolist(),
                "state_std": dataset.state_std.tolist(),
                "action_mean": dataset.action_mean.tolist(),
                "action_std": dataset.action_std.tolist(),
            }
        )
        dataset.write_checkpoint(
            self.job_id,
            str(request["config"]["run_name"]),
            step,
            _now_ms(),
            validation_loss,
            json.dumps(model_config, separators=(",", ":")),
            payload,
        )
        with self.lock:
            self.logs.append(
                f"Checkpoint {step} committed directly to "
                f"{dataset.table_prefix}_training_checkpoints."
            )


def _session_request(config: dict[str, Any], dataset: McapTrainingDataset) -> dict[str, Any]:
    act = config["act"]
    return {
        "model": {
            "policy": "act",
            "state_dimensions": dataset.state_dimensions,
            "action_dimensions": dataset.action_dimensions,
            "camera_streams": list(dataset.camera_streams),
            "image_width": dataset.image_width,
            "image_height": dataset.image_height,
            "chunk_size": config["chunk_size"],
            "hidden_dimensions": act["hidden_dim"],
            "latent_dimensions": act["latent_dim"],
            "attention_heads": act["attention_heads"],
            "feedforward_dimensions": act["feedforward_dim"],
            "encoder_layers": act["encoder_layers"],
            "vae_encoder_layers": act["vae_encoder_layers"],
            "decoder_layers": act["decoder_layers"],
            "kl_weight": act["kl_weight"],
            "learning_rate": config["learning_rate"],
            "seed": config["seed"],
        },
        "device": config["device"],
        "initialization": "pretrained_backbone" if act["pretrained_backbone"] else "random",
    }


def _checkpoint_manifest(
    request: dict[str, Any],
    dataset: McapTrainingDataset,
    session: TrainingSession,
    step: int,
) -> tuple[dict[str, Any], dict[str, Any]]:
    config = request["config"]
    act = config["act"]
    backend = backend_info()
    input_features: dict[str, Any] = {
        "observation.state": {"type": "STATE", "shape": [dataset.state_dimensions]}
    }
    for camera in dataset.camera_streams:
        input_features[camera] = {
            "type": "VISUAL",
            "shape": [3, dataset.image_height, dataset.image_width],
        }
    model_config = {
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD",
        },
        "input_features": input_features,
        "output_features": {"action": {"type": "ACTION", "shape": [dataset.action_dimensions]}},
        "chunk_size": config["chunk_size"],
        "n_action_steps": min(config["chunk_size"], MAX_ROLLOUT_ACTION_STEPS),
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": False,
        "pre_norm": False,
        "dim_model": act["hidden_dim"],
        "n_heads": act["attention_heads"],
        "dim_feedforward": act["feedforward_dim"],
        "feedforward_activation": "relu",
        "n_encoder_layers": act["encoder_layers"],
        "n_vae_encoder_layers": act["vae_encoder_layers"],
        "n_decoder_layers": act["decoder_layers"],
        "use_vae": True,
        "latent_dim": act["latent_dim"],
        "spiral_camera_embedding": False,
        "spiral_camera_order": list(dataset.camera_streams),
        "spiral_sample_hz": dataset.sample_hz,
    }
    manifest = {
        "format": "spiral_robotics.act.safetensors.v2",
        "artifact": "inference",
        "run_id": request["job_id"],
        "step": step,
        "data_contract": {
            "value_space": "centered_normalized",
            "robot_type": dataset.robot_type,
            "joint_names": list(dataset.joint_names),
            "camera_streams": list(dataset.camera_streams),
            "sample_hz": dataset.sample_hz,
            "image_layout": "chw",
            "image_normalization": "imagenet",
            "frame_alignment": "fixed_fps",
            "spiral_schema_version": int(request["spiral_schema_version"]),
        },
        "runtime": {
            "training_backend": {
                "api_id": backend["api_id"],
                "api_version": backend["api_version"],
                "api_revision": backend["api_revision"],
                "id": backend["id"],
                "runtime": backend["runtime"],
                "device": session.device_name,
            },
            "transport": "direct SpiralDB ADBC / Arrow",
            "language": "python",
        },
        "normalization": "embedded_mean_std",
        "model_config": model_config,
        "tensor_namespace": "lerobot_act_state_dict_v1",
    }
    return manifest, model_config


def _now_ms() -> int:
    return int(time.time() * 1_000)
