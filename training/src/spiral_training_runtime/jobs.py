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
from .spiral import SpiralActDataset

MAX_ROLLOUT_ACTION_STEPS = 100
MAX_METRIC_HISTORY = 4_096
MAX_CHECKPOINT_HISTORY = 256
MAX_LOG_HISTORY = 512


class TrainingJob:
    """A worker-owned data/model/checkpoint lifecycle with replayable state."""

    def __init__(self, request: dict[str, Any]) -> None:
        job_id = request.get("job_id")
        if not isinstance(job_id, str) or not job_id.strip():
            raise RuntimeRequestError("training job_id must be non-empty", 422)
        self.job_id = job_id
        self.request_digest = hashlib.sha256(
            json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
        ).digest()
        self.lock = threading.Lock()
        self.stop_event = threading.Event()
        self.phase = "starting"
        self.current_step = 0
        self.finished_ms: int | None = None
        self.message: str | None = "Opening SpiralDB and resolving the ACT data contract…"
        self.resolved: dict[str, Any] | None = None
        self.metrics: deque[dict[str, Any]] = deque(maxlen=MAX_METRIC_HISTORY)
        self.checkpoints: deque[dict[str, Any]] = deque(maxlen=MAX_CHECKPOINT_HISTORY)
        self.logs: deque[dict[str, Any]] = deque(maxlen=MAX_LOG_HISTORY)
        self.metric_floor = 0
        self.checkpoint_floor = 0
        self.log_floor = 0
        self.next_log_offset = 1
        self.secret = str(request.get("spiral", {}).get("client_secret") or "")
        self.thread = threading.Thread(
            target=self._run, args=(request,), name=f"training-{job_id}", daemon=True
        )

    def start(self) -> None:
        self.thread.start()

    def matches(self, request: dict[str, Any]) -> bool:
        digest = hashlib.sha256(
            json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
        ).digest()
        return digest == self.request_digest

    def stop(self) -> None:
        self.stop_event.set()
        with self.lock:
            if self.phase in {"starting", "running"}:
                self.phase = "stopping"
                self.message = "Finishing the current Spiral read or optimizer step…"

    def terminal(self) -> bool:
        with self.lock:
            return self.phase in {"completed", "stopped", "failed"}

    def snapshot(
        self,
        after_metric_step: int = 0,
        after_checkpoint_step: int = 0,
        after_log_offset: int = 0,
    ) -> dict[str, Any]:
        with self.lock:
            gaps = []
            if after_metric_step < self.metric_floor:
                gaps.append(f"metrics through step {self.metric_floor} are no longer retained")
            if after_checkpoint_step < self.checkpoint_floor:
                gaps.append(
                    f"checkpoints through step {self.checkpoint_floor} are no longer retained"
                )
            if after_log_offset < self.log_floor:
                gaps.append(f"logs through offset {self.log_floor} are no longer retained")
            metric_step = self.metrics[-1]["step"] if self.metrics else self.metric_floor
            checkpoint_step = (
                self.checkpoints[-1]["step"] if self.checkpoints else self.checkpoint_floor
            )
            return {
                "job_id": self.job_id,
                "phase": self.phase,
                "current_step": self.current_step,
                "finished_ms": self.finished_ms,
                "message": self.message,
                "resolved": dict(self.resolved) if self.resolved is not None else None,
                "metrics": [
                    dict(metric) for metric in self.metrics if metric["step"] > after_metric_step
                ],
                "checkpoints": [
                    dict(checkpoint)
                    for checkpoint in self.checkpoints
                    if checkpoint["step"] > after_checkpoint_step
                ],
                "logs": [dict(log) for log in self.logs if log["offset"] > after_log_offset],
                "cursor": {
                    "metric_step": metric_step,
                    "checkpoint_step": checkpoint_step,
                    "log_offset": self.next_log_offset - 1,
                },
                "history_gap": "; ".join(gaps) if gaps else None,
            }

    def _append_metric_locked(self, metric: dict[str, Any]) -> None:
        if len(self.metrics) == MAX_METRIC_HISTORY:
            self.metric_floor = int(self.metrics[0]["step"])
        self.metrics.append(metric)

    def _append_checkpoint_locked(self, checkpoint: dict[str, Any]) -> None:
        if len(self.checkpoints) == MAX_CHECKPOINT_HISTORY:
            self.checkpoint_floor = int(self.checkpoints[0]["step"])
        self.checkpoints.append(checkpoint)

    def _append_log_locked(self, line: str) -> None:
        if len(self.logs) == MAX_LOG_HISTORY:
            self.log_floor = int(self.logs[0]["offset"])
        self.logs.append({"offset": self.next_log_offset, "line": line})
        self.next_log_offset += 1

    def _run(self, request: dict[str, Any]) -> None:
        dataset = None
        try:
            dataset = SpiralActDataset(request)
            config = TrainingConfig.from_request(_session_request(request, dataset))
            session = TrainingSession(config)
            resolved = {
                "camera_streams": list(dataset.camera_streams),
                "state_dimensions": dataset.state_dimensions,
                "action_dimensions": dataset.action_dimensions,
                "image_width": dataset.image_width,
                "image_height": dataset.image_height,
                "sample_hz": dataset.sample_hz,
                "robot_type": dataset.robot_type,
                "joint_names": list(dataset.joint_names),
                "parameter_count": session.parameter_count,
                "device": session.info["device"],
            }
            with self.lock:
                self.resolved = resolved
                self.phase = "running"
                self.message = (
                    f"Vision ACT is running on {session.info['device']}: {dataset.summary}; "
                    f"{session.parameter_count / 1_000_000.0:.1f}M parameters."
                )
                self._append_log_locked(
                    "The worker owns SpiralDB reads, SPQL image decode, model execution, "
                    "and checkpoint writes; Studio carries no training batches."
                )
            validation_every = min(250, max(1, math.ceil(int(request["checkpoint_every"]) / 4)))
            started = time.perf_counter()
            last_validation: float | None = None
            last_checkpoint = 0
            for step in range(1, int(request["steps"]) + 1):
                if self.stop_event.is_set():
                    break
                arrays, loader_ms = dataset.fetch(False)
                step_started = time.perf_counter()
                session.buffer_batch(arrays, "training", step)
                response = session.train_buffered(step)
                validation_loss = None
                if dataset.has_validation and step % validation_every == 0:
                    validation, validation_loader_ms = dataset.fetch(True)
                    loader_ms += validation_loader_ms
                    session.buffer_batch(validation, "validation", step)
                    validation_response = session.validate_buffered(step)
                    validation_loss = float(validation_response["loss"]["total"])
                    last_validation = validation_loss
                elapsed = time.perf_counter() - started
                step_seconds = max(time.perf_counter() - step_started, 1.0e-9)
                telemetry = response["telemetry"]
                metric = {
                    "step": step,
                    "train_loss": float(response["loss"]["total"]),
                    "validation_loss": validation_loss,
                    "learning_rate": float(response["learning_rate"]),
                    "examples_per_second": int(request["batch_size"]) / step_seconds,
                    "cpu_percent": telemetry.get("cpu_percent"),
                    "gpu_percent": telemetry.get("gpu_percent"),
                    "memory_bytes": telemetry.get("memory_bytes"),
                    "loader_ms": loader_ms,
                    "device_copy_ms": telemetry.get("device_copy_ms"),
                    "model_ms": telemetry.get("model_ms"),
                    "elapsed_seconds": elapsed,
                    "event": "validation" if validation_loss is not None else None,
                }
                with self.lock:
                    self.current_step = step
                    self._append_metric_locked(metric)
                if step % int(request["checkpoint_every"]) == 0:
                    self._checkpoint(dataset, session, request, step, last_validation)
                    last_checkpoint = step
            completed = self.current_step
            if completed > 0 and last_checkpoint != completed:
                self._checkpoint(dataset, session, request, completed, last_validation)
            with self.lock:
                stopped = self.stop_event.is_set()
                self.phase = "stopped" if stopped else "completed"
                self.finished_ms = _now_ms()
                self.message = (
                    f"Stopped after {completed} optimizer step(s)."
                    if stopped
                    else f"Completed {completed} optimizer step(s)."
                )
        except Exception as error:
            detail = str(error).replace(self.secret, "[redacted]") if self.secret else str(error)
            with self.lock:
                self.phase = "failed"
                self.finished_ms = _now_ms()
                self.message = detail
                self._append_log_locked(f"Training failed: {detail}")
        finally:
            self.secret = ""
            request.get("spiral", {}).pop("client_secret", None)
            if dataset is not None:
                dataset.close()

    def _checkpoint(
        self,
        dataset: SpiralActDataset,
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
        created_ms = _now_ms()
        dataset.write_checkpoint(
            self.job_id,
            str(request["run_name"]),
            step,
            created_ms,
            validation_loss,
            json.dumps(model_config, separators=(",", ":")),
            payload,
        )
        checkpoint = {
            "step": step,
            "created_ms": created_ms,
            "validation_loss": validation_loss,
            "byte_size": len(payload),
            "recommended_action_steps": min(int(request["chunk_size"]), MAX_ROLLOUT_ACTION_STEPS),
            "sample_hz": dataset.sample_hz,
        }
        with self.lock:
            self._append_checkpoint_locked(checkpoint)
            self._append_log_locked(
                f"Checkpoint {step} committed directly to "
                f"{dataset.table_prefix}_training_checkpoints."
            )


def _session_request(request: dict[str, Any], dataset: SpiralActDataset) -> dict[str, Any]:
    act = request["act"]
    return {
        "model": {
            "policy": "act",
            "state_dimensions": dataset.state_dimensions,
            "action_dimensions": dataset.action_dimensions,
            "camera_streams": list(dataset.camera_streams),
            "image_width": dataset.image_width,
            "image_height": dataset.image_height,
            "chunk_size": request["chunk_size"],
            "hidden_dimensions": act["hidden_dimensions"],
            "latent_dimensions": act["latent_dimensions"],
            "attention_heads": act["attention_heads"],
            "feedforward_dimensions": act["feedforward_dimensions"],
            "encoder_layers": act["encoder_layers"],
            "vae_encoder_layers": act["vae_encoder_layers"],
            "decoder_layers": act["decoder_layers"],
            "kl_weight": act["kl_weight"],
            "learning_rate": request["learning_rate"],
            "total_steps": request["steps"],
            "seed": request["seed"],
        },
        "device": request["device"],
        "initialization": request["initialization"],
    }


def _checkpoint_manifest(
    request: dict[str, Any],
    dataset: SpiralActDataset,
    session: TrainingSession,
    step: int,
) -> tuple[dict[str, Any], dict[str, Any]]:
    act = request["act"]
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
        "chunk_size": request["chunk_size"],
        "n_action_steps": min(request["chunk_size"], MAX_ROLLOUT_ACTION_STEPS),
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": False,
        "pre_norm": False,
        "dim_model": act["hidden_dimensions"],
        "n_heads": act["attention_heads"],
        "dim_feedforward": act["feedforward_dimensions"],
        "feedforward_activation": "relu",
        "n_encoder_layers": act["encoder_layers"],
        "n_vae_encoder_layers": act["vae_encoder_layers"],
        "n_decoder_layers": act["decoder_layers"],
        "use_vae": True,
        "latent_dim": act["latent_dimensions"],
        "spiral_camera_embedding": False,
        "spiral_camera_order": list(dataset.camera_streams),
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
                "device": session.info["device"],
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
