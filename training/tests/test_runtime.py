from __future__ import annotations

import asyncio
import gc
import json
import math
import threading
import time

import httpx
import numpy as np
import pyarrow as pa
import pytest
import torch

from spiral_training_runtime.act import (
    IMAGENET_MEAN,
    IMAGENET_STD,
    TrainingSession,
    _gpu_utilization,
    _normalize_hwc_images,
)
from spiral_training_runtime.arrow import decode_training_batch
from spiral_training_runtime.config import TrainingConfig
from spiral_training_runtime.service import (
    ARROW_STREAM_CONTENT_TYPE,
    POLICY_ARTIFACT_CONTENT_TYPE,
    create_app,
)
from spiral_training_runtime.spiral import Sample, SpiralActDataset, VideoSegment, connect_spiral


def training_request() -> dict:
    return {
        "model": {
            "policy": "act",
            "state_dimensions": 2,
            "action_dimensions": 2,
            "camera_streams": ["observation.images.wrist"],
            "image_width": 32,
            "image_height": 32,
            "chunk_size": 2,
            "hidden_dimensions": 16,
            "latent_dimensions": 4,
            "attention_heads": 4,
            "feedforward_dimensions": 32,
            "encoder_layers": 1,
            "vae_encoder_layers": 1,
            "decoder_layers": 1,
            "kl_weight": 1.0,
            "learning_rate": 1.0e-5,
            "total_steps": 2,
            "seed": 7,
        },
        "device": "cpu",
        "initialization": "random",
    }


def job_request() -> dict:
    return {
        "job_id": "train-test",
        "run_name": "test",
        "spiral": {
            "api_url": "https://api.spiraldb.dev",
            "spfs_url": "https://spfs.spiraldb.dev",
            "project_id": "project-test",
            "namespace": "spiral_robotics",
            "client_id": "client-id",
            "client_secret": "client-secret",
        },
        "sql": "SELECT * FROM spiral_robotics.joint_samples",
        "state_column": "state",
        "action_column": "action",
        "device": "cpu",
        "initialization": "random",
        "steps": 1,
        "batch_size": 1,
        "validation_split": 0.0,
        "learning_rate": 1.0e-5,
        "checkpoint_every": 1,
        "chunk_size": 2,
        "seed": 7,
        "act": {
            "camera_streams": ["observation.images.wrist"],
            "image_width": 32,
            "image_height": 32,
            "video_decoder": "auto",
            "hidden_dimensions": 16,
            "latent_dimensions": 4,
            "attention_heads": 4,
            "feedforward_dimensions": 32,
            "encoder_layers": 1,
            "vae_encoder_layers": 1,
            "decoder_layers": 1,
            "kl_weight": 1.0,
        },
    }


def test_spiral_connection_uses_scoped_adbc_options(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict = {}

    class Native:
        def close(self) -> None:
            pass

    class Connection:
        def close(self) -> None:
            pass

    monkeypatch.delenv("SPIRALDB_ADBC_DRIVER", raising=False)
    monkeypatch.setattr("spiral_training_runtime.spiral.driver_path", lambda: "/driver.dylib")
    monkeypatch.setattr(
        "spiral_training_runtime.spiral.adbc_driver_manager.AdbcDatabase",
        lambda **options: captured.update(options) or Native(),
    )
    monkeypatch.setattr(
        "spiral_training_runtime.spiral.adbc_driver_manager.AdbcConnection", lambda _db: Native()
    )
    monkeypatch.setattr(
        "spiral_training_runtime.spiral.dbapi.Connection", lambda *_args, **_kwargs: Connection()
    )

    connection = connect_spiral(job_request()["spiral"])
    connection.close()

    assert captured == {
        "driver": "/driver.dylib",
        "uri": "spiraldb://?project=project-test",
        "spiral.api_url": "https://api.spiraldb.dev",
        "spiral.spfs_url": "https://spfs.spiraldb.dev",
        "username": "client-id",
        "password": "client-secret",
    }


def test_training_anchors_follow_camera_frames_instead_of_joint_rows() -> None:
    dataset = object.__new__(SpiralActDataset)
    dataset.camera_streams = ("observation.images.wrist",)
    dataset.sample_hz = 30.0
    dataset.joint_sample_hz = 60.0
    samples = {
        index: Sample(
            episode_id="episode",
            stream_id="follower",
            sample_index=index,
            episode_time_ns=round(index * 1_000_000_000 / 60),
            sample_hz=60.0,
            state=np.array([index], dtype=np.float32),
            action=np.array([index], dtype=np.float32),
            robot_type="so101_follower",
            joint_names=("joint",),
        )
        for index in range(6)
    }
    dataset.by_stream = {("episode", "follower"): samples}
    dataset.segments = [
        VideoSegment(
            episode_id="episode",
            stream_id="observation.images.wrist",
            segment_index=0,
            timeline_offset_ns=0,
            fps=30,
            frames=3,
            width=32,
            height=32,
            media_relation="spiral_robotics.video_media",
        )
    ]

    anchors = dataset._build_anchors()

    assert [anchor.frame_index for anchor in anchors] == [0, 1, 2]
    assert [anchor.sample.sample_index for anchor in anchors] == [0, 2, 4]


def test_job_api_owns_data_training_and_checkpoint_lifecycle(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    writes: list[tuple] = []

    class Dataset:
        camera_streams = ("observation.images.wrist",)
        state_dimensions = 2
        action_dimensions = 2
        image_width = 32
        image_height = 32
        sample_hz = 30.0
        robot_type = "so101_follower"
        joint_names = ("shoulder", "elbow")
        state_mean = np.zeros(2, dtype=np.float32)
        state_std = np.ones(2, dtype=np.float32)
        action_mean = np.zeros(2, dtype=np.float32)
        action_std = np.ones(2, dtype=np.float32)
        has_validation = False
        summary = "one test anchor"
        namespace = "spiral_robotics"

        def __init__(self, _request: dict) -> None:
            pass

        def fetch(self, _validation: bool = False) -> tuple[object, float]:
            return object(), 1.0

        def write_checkpoint(self, *args: object) -> None:
            writes.append(args)

        def close(self) -> None:
            pass

    class Session:
        parameter_count = 10

        def __init__(self, _config: TrainingConfig) -> None:
            self.info = {"device": "CPU"}

        def buffer_batch(self, _arrays: object, _kind: str, _step: int) -> None:
            pass

        def train_buffered(self, _step: int) -> dict:
            return {
                "loss": {"total": 0.25},
                "learning_rate": 1.0e-5,
                "telemetry": {
                    "cpu_percent": 1.0,
                    "gpu_percent": None,
                    "memory_bytes": 100,
                    "device_copy_ms": 2.0,
                    "model_ms": 3.0,
                },
            }

        def checkpoint(self, _request: dict) -> bytes:
            return b"safe-tensors"

    monkeypatch.setattr("spiral_training_runtime.jobs.SpiralActDataset", Dataset)
    monkeypatch.setattr("spiral_training_runtime.jobs.TrainingSession", Session)

    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app())
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            created = await client.post("/v1/jobs", json=job_request())
            assert created.status_code == 201
            duplicate = job_request()
            duplicate["run_name"] = "different"
            conflict = await client.post("/v1/jobs", json=duplicate)
            assert conflict.status_code == 409
            assert "immutable specification" in conflict.json()["error"]
            deadline = time.monotonic() + 2.0
            snapshot = created.json()
            while snapshot["phase"] not in {"completed", "failed"}:
                assert time.monotonic() < deadline
                await asyncio.sleep(0.01)
                snapshot = (await client.get("/v1/jobs/train-test")).json()
            assert snapshot["phase"] == "completed", snapshot
            assert snapshot["current_step"] == 1
            assert len(snapshot["metrics"]) == 1
            assert len(snapshot["checkpoints"]) == 1
            assert "client-secret" not in json.dumps(snapshot)

    asyncio.run(exercise())
    assert len(writes) == 1


def policy_config() -> dict:
    return {
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD",
        },
        "input_features": {
            "observation.images.wrist": {"type": "VISUAL", "shape": [3, 32, 32]},
            "observation.state": {"type": "STATE", "shape": [2]},
        },
        "output_features": {"action": {"type": "ACTION", "shape": [2]}},
        "chunk_size": 2,
        "n_action_steps": 2,
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": False,
        "pre_norm": False,
        "dim_model": 16,
        "n_heads": 4,
        "dim_feedforward": 32,
        "feedforward_activation": "relu",
        "n_encoder_layers": 1,
        "n_vae_encoder_layers": 1,
        "n_decoder_layers": 1,
        "use_vae": True,
        "latent_dim": 4,
        "spiral_camera_embedding": False,
        "spiral_camera_order": ["observation.images.wrist"],
    }


def fixed_list(values: np.ndarray, rows: int) -> pa.FixedSizeListArray:
    flat = pa.array(values.reshape(-1))
    return pa.FixedSizeListArray.from_arrays(flat, len(values.reshape(rows, -1)[0]))


def arrow_stream(columns: dict[str, pa.Array]) -> bytes:
    batch = pa.RecordBatch.from_arrays(list(columns.values()), names=list(columns))
    sink = pa.BufferOutputStream()
    options = pa.ipc.IpcWriteOptions(compression="zstd")
    with pa.ipc.new_stream(sink, batch.schema, options=options) as writer:
        writer.write_batch(batch)
    return sink.getvalue().to_pybytes()


def training_body() -> bytes:
    return arrow_stream(
        {
            "states": fixed_list(np.zeros((1, 2), dtype=np.float32), 1),
            "images": fixed_list(np.zeros((1, 3 * 32 * 32), dtype=np.uint8), 1),
            "actions": fixed_list(np.zeros((1, 4), dtype=np.float32), 1),
            "action_is_pad": fixed_list(np.zeros((1, 2), dtype=np.uint8), 1),
        }
    )


def inference_body() -> bytes:
    return arrow_stream(
        {
            "state": fixed_list(np.zeros((1, 2), dtype=np.float32), 1),
            "image_0": fixed_list(np.zeros((1, 3 * 32 * 32), dtype=np.uint8), 1),
        }
    )


def test_arrow_values_are_zero_copy_numpy_views() -> None:
    config = TrainingConfig.from_request(training_request())
    body = training_body()
    assert len(body) < 3 * 32 * 32
    batch = decode_training_batch(body, config)
    assert batch.states.shape == (1, 2)
    assert batch.images.shape == (1, 3 * 32 * 32)
    assert not batch.states.flags["OWNDATA"]
    assert not batch.images.flags["OWNDATA"]


def test_http_surface_reports_pytorch_without_constructing_a_model() -> None:
    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app())
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            response = await client.get("/v1/health")
            assert response.status_code == 200
            assert response.json()["api_version"] == 4
            assert response.json()["id"] == "pytorch.lerobot"

            response = await client.post(
                "/v1/sessions/missing/train?step=1",
            )
            assert response.status_code == 404
            assert "does not exist" in response.json()["error"]

    asyncio.run(exercise())


def test_missing_nvml_never_fails_gpu_telemetry(monkeypatch: pytest.MonkeyPatch) -> None:
    def missing_nvml(_device: torch.device) -> int:
        raise ModuleNotFoundError("pynvml is unavailable")

    monkeypatch.setattr(torch.cuda, "utilization", missing_nvml)
    assert _gpu_utilization(torch.device("cuda:0")) is None


def test_hwc_uint8_training_images_match_imagenet_chw_normalization() -> None:
    images = torch.tensor([255, 128, 0, 0, 64, 255], dtype=torch.uint8).reshape(1, 1, 1, 2, 3)
    mean = torch.tensor(IMAGENET_MEAN).reshape(1, 1, 3, 1, 1)
    std = torch.tensor(IMAGENET_STD).reshape(1, 1, 3, 1, 1)

    actual = _normalize_hwc_images(images, mean, std).reshape(3, 2)
    expected = torch.tensor(
        [
            [(1.0 - 0.485) / 0.229, (0.0 - 0.485) / 0.229],
            [(128.0 / 255.0 - 0.456) / 0.224, (64.0 / 255.0 - 0.456) / 0.224],
            [(0.0 - 0.406) / 0.225, (1.0 - 0.406) / 0.225],
        ]
    )
    torch.testing.assert_close(actual, expected, rtol=1.0e-6, atol=1.0e-6)


def test_session_buffer_holds_exactly_one_batch_ahead() -> None:
    config = TrainingConfig.from_request(training_request())
    first = decode_training_batch(training_body(), config)
    second = decode_training_batch(training_body(), config)
    session = object.__new__(TrainingSession)
    session.buffer_condition = threading.Condition()
    session.buffered_batch = None
    started = threading.Event()
    finished = threading.Event()
    errors: list[Exception] = []

    session.buffer_batch(first, "training", 1)

    def buffer_second() -> None:
        started.set()
        try:
            session.buffer_batch(second, "training", 2)
        except Exception as error:  # pragma: no cover - asserted below
            errors.append(error)
        finally:
            finished.set()

    worker = threading.Thread(target=buffer_second)
    worker.start()
    assert started.wait(1.0)
    assert not finished.wait(0.05)
    assert session._take_buffered_batch("training", 1) is first
    assert finished.wait(1.0)
    worker.join()
    assert not errors
    assert session._take_buffered_batch("training", 2) is second


def test_real_pytorch_act_train_checkpoint_and_infer_round_trip() -> None:
    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app(policy_device="cpu"))
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            created = await client.post("/v1/sessions", json=training_request())
            assert created.status_code == 201, created.text
            session_id = created.json()["session_id"]
            assert created.json()["info"]["backend"]["id"] == "pytorch.lerobot"

            buffered = await client.post(
                f"/v1/sessions/{session_id}/buffer?kind=training&step=1",
                content=training_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert buffered.status_code == 202, buffered.text
            assert math.isfinite(buffered.json()["runtime_ms"])

            first_step = asyncio.create_task(client.post(f"/v1/sessions/{session_id}/train?step=1"))
            await asyncio.sleep(0)
            buffered_next = await client.post(
                f"/v1/sessions/{session_id}/buffer?kind=training&step=2",
                content=training_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert buffered_next.status_code == 202, buffered_next.text

            trained = await first_step
            assert trained.status_code == 200, trained.text
            trained_body = trained.json()
            assert math.isfinite(trained_body["loss"]["total"])
            assert math.isfinite(trained_body["runtime_ms"])
            assert trained_body["runtime_ms"] >= 0.0
            assert math.isfinite(trained_body["telemetry"]["device_copy_ms"])
            assert trained_body["telemetry"]["device_copy_ms"] >= 0.0
            assert math.isfinite(trained_body["telemetry"]["model_ms"])
            assert trained_body["telemetry"]["model_ms"] >= 0.0

            trained_next = await client.post(f"/v1/sessions/{session_id}/train?step=2")
            assert trained_next.status_code == 200, trained_next.text
            assert math.isfinite(trained_next.json()["loss"]["total"])

            checkpoint_response = await client.post(
                f"/v1/sessions/{session_id}/checkpoint",
                json={
                    "manifest_json": "{}",
                    "state_mean": [0.0, 0.0],
                    "state_std": [1.0, 1.0],
                    "action_mean": [0.0, 0.0],
                    "action_std": [1.0, 1.0],
                },
            )
            assert checkpoint_response.status_code == 200, checkpoint_response.text
            assert checkpoint_response.headers["x-spiral-checkpoint-format"] == "safetensors"
            checkpoint = checkpoint_response.content
            assert len(checkpoint) > 1_000

            deleted = await client.delete(f"/v1/sessions/{session_id}")
            assert deleted.status_code == 204
            gc.collect()

            config_bytes = json.dumps(policy_config(), separators=(",", ":")).encode()
            artifact = len(config_bytes).to_bytes(8, "little") + config_bytes + checkpoint
            policy_created = await client.post(
                "/v1/policies",
                content=artifact,
                headers={"content-type": POLICY_ARTIFACT_CONTENT_TYPE},
            )
            assert policy_created.status_code == 201, policy_created.text
            policy_id = policy_created.json()["session_id"]
            assert policy_created.json()["specification"]["kind"] == "act"

            inferred = await client.post(
                f"/v1/policies/{policy_id}/infer",
                content=inference_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert inferred.status_code == 200, inferred.text
            result = inferred.json()
            assert result["steps"] == 2
            assert result["action_dimensions"] == 2
            assert len(result["values"]) == 4
            assert all(math.isfinite(value) for value in result["values"])

    asyncio.run(exercise())
