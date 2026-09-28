from __future__ import annotations

import asyncio
import gc
import json
import math
import time
from itertools import pairwise
from pathlib import Path
from types import SimpleNamespace

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
from spiral_training_runtime.arrow import TrainingArrays, decode_inference_input
from spiral_training_runtime.config import PolicyConfig, TrainingConfig
from spiral_training_runtime.errors import RuntimeRequestError
from spiral_training_runtime.jobs import (
    MetricSeries,
    TrainingJob,
    _checkpoint_manifest,
)
from spiral_training_runtime.mcap import McapTrainingDataset
from spiral_training_runtime.models import (
    TrainingBackendInfoModel,
    TrainingJobHistoryModel,
    TrainingJobRequestModel,
)
from spiral_training_runtime.protocol import VERSIONS
from spiral_training_runtime.service import (
    ARROW_STREAM_CONTENT_TYPE,
    POLICY_ARTIFACT_CONTENT_TYPE,
    create_app,
)

CANONICAL_FIXTURES = (
    Path(__file__).resolve().parents[2] / "crates" / "backend" / "training" / "fixtures"
)


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
            "seed": 7,
        },
        "device": "cpu",
        "initialization": "random",
    }


def job_request() -> dict:
    return {
        "job_id": "train-test",
        "spiral": {
            "api_url": "https://api.spiraldb.dev",
            "spfs_url": "https://spfs.spiraldb.dev",
            "project_id": "project-test",
            "table_prefix": "spiral_robotics",
        },
        "spiral_schema_version": VERSIONS.storage_schema,
        "config": {
            "run_name": "test",
            "dataset_id": "fixture",
            "policy": "act",
            "sql": "SELECT * FROM fixture",
            "state_column": "state",
            "action_column": "action",
            "device": "cpu",
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
                "pretrained_backbone": False,
                "hidden_dim": 16,
                "latent_dim": 4,
                "encoder_layers": 1,
                "vae_encoder_layers": 1,
                "decoder_layers": 1,
                "attention_heads": 4,
                "feedforward_dim": 32,
                "kl_weight": 1.0,
            },
        },
    }


@pytest.mark.parametrize(
    ("fixture", "model"),
    [
        ("health.json", TrainingBackendInfoModel),
        ("job-request.json", TrainingJobRequestModel),
        ("job-history.json", TrainingJobHistoryModel),
    ],
)
def test_canonical_wire_fixtures_round_trip_through_python_models(
    fixture: str, model: type
) -> None:
    source = (CANONICAL_FIXTURES / fixture).read_text(encoding="utf-8")
    expected = json.loads(source)

    actual = model.model_validate(expected).model_dump(mode="json")

    assert actual == expected


def test_canonical_fixture_versions_follow_the_boundary_descriptor() -> None:
    health = json.loads((CANONICAL_FIXTURES / "health.json").read_text(encoding="utf-8"))
    request = json.loads((CANONICAL_FIXTURES / "job-request.json").read_text(encoding="utf-8"))

    assert health["api_id"] == VERSIONS.training_protocol.id
    assert health["api_version"] == VERSIONS.training_protocol.major
    assert health["api_revision"] == VERSIONS.training_protocol.minor
    assert health["spiral_schema_minimum"] == VERSIONS.training_protocol.storage_schema.minimum
    assert health["spiral_schema_maximum"] == VERSIONS.training_protocol.storage_schema.maximum
    assert request["spiral_schema_version"] == VERSIONS.storage_schema


def test_mcap_training_requires_native_spiral_image_decoding() -> None:
    with pytest.raises(RuntimeRequestError, match="MCAP camera training is unavailable"):
        McapTrainingDataset()


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
        table_prefix = "spiral_robotics"

        def fetch(self, _validation: bool = False) -> tuple[object, float]:
            return object(), 1.0

        def write_checkpoint(self, *args: object) -> None:
            writes.append(args)

        def close(self) -> None:
            pass

    class Session:
        parameter_count = 10
        device_name = "CPU"

        def __init__(self, _config: TrainingConfig) -> None:
            pass

        def train_step(self, _arrays: object, _step: int) -> dict:
            return {
                "loss": 0.25,
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

    monkeypatch.setattr("spiral_training_runtime.jobs.McapTrainingDataset", Dataset)
    monkeypatch.setattr("spiral_training_runtime.jobs.TrainingSession", Session)

    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app())
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            created = await client.post("/v1/jobs", json=job_request())
            assert created.status_code == 201
            duplicate = job_request()
            duplicate["config"]["run_name"] = "different"
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
            assert any("Checkpoint 1 committed" in line for line in snapshot["logs"])
            listed = (await client.get("/v1/jobs")).json()
            assert listed == [
                {
                    "job_id": "train-test",
                    "phase": "completed",
                    "current_step": 1,
                    "message": snapshot["message"],
                }
            ]
            missing = await client.get("/v1/jobs/missing")
            assert missing.status_code == 404

            invalid = job_request()
            invalid["unexpected"] = True
            rejected = await client.post("/v1/jobs", json=invalid)
            assert rejected.status_code == 422

            incompatible = job_request()
            incompatible["spiral_schema_version"] = (
                VERSIONS.training_protocol.storage_schema.maximum + 1
            )
            rejected = await client.post("/v1/jobs", json=incompatible)
            assert rejected.status_code == 422

    asyncio.run(exercise())
    assert len(writes) == 1


def test_metric_series_stays_bounded_and_spans_the_whole_run() -> None:
    series = MetricSeries(8)
    for step in range(1, 101):
        series.append({"step": step})

    steps = [point["step"] for point in series.points]

    assert len(steps) <= 8
    assert steps[0] == 1
    assert steps[-1] > 80
    gaps = {later - earlier for earlier, later in pairwise(steps)}
    assert len(gaps) == 1, steps


def test_job_history_keeps_validation_points_apart_from_step_samples() -> None:
    job = TrainingJob(job_request())
    with job.lock:
        for step in range(1, 5):
            job._append_metric_locked({"step": step, "event": None})
        job._append_metric_locked({"step": 5, "event": "validation"})

    history = job.history()

    assert [metric["step"] for metric in history["metrics"]] == [1, 2, 3, 4, 5]
    assert history["logs"] == []
    assert history["phase"] == "starting"


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


def inference_body() -> bytes:
    return arrow_stream(
        {
            "state": fixed_list(np.zeros((1, 2), dtype=np.float32), 1),
            "image_0": fixed_list(np.zeros((1, 3 * 32 * 32), dtype=np.uint8), 1),
        }
    )


def test_arrow_values_are_zero_copy_numpy_views() -> None:
    config = PolicyConfig.from_json(policy_config())
    body = inference_body()
    assert len(body) < 3 * 32 * 32
    arrays = decode_inference_input(body, config)
    assert arrays.state.shape == (1, 2)
    assert arrays.images[0].shape == (1, 3 * 32 * 32)
    assert not arrays.state.flags["OWNDATA"]
    assert not arrays.images[0].flags["OWNDATA"]


def test_http_surface_reports_pytorch_without_constructing_a_model() -> None:
    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app())
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            response = await client.get("/v1/health")
            assert response.status_code == 200
            health = response.json()
            assert health["api_id"] == VERSIONS.training_protocol.id
            assert health["api_version"] == VERSIONS.training_protocol.major
            assert health["api_revision"] == VERSIONS.training_protocol.minor
            assert (
                health["spiral_schema_minimum"] == VERSIONS.training_protocol.storage_schema.minimum
            )
            assert (
                health["spiral_schema_maximum"] == VERSIONS.training_protocol.storage_schema.maximum
            )
            assert health["id"] == "pytorch.lerobot"

            response = await client.get("/openapi.json")
            assert response.status_code == 200
            assert response.json()["info"]["version"] == VERSIONS.training_protocol.display_version

    asyncio.run(exercise())


def test_checkpoint_manifest_carries_the_job_schema_version() -> None:
    request = job_request()
    dataset = SimpleNamespace(
        robot_type="so101_follower",
        joint_names=("joint",),
        camera_streams=("observation.images.wrist",),
        sample_hz=30.0,
        state_dimensions=1,
        action_dimensions=1,
        image_height=32,
        image_width=32,
    )
    session = SimpleNamespace(device_name="cpu")

    manifest, _model = _checkpoint_manifest(request, dataset, session, 1)

    assert manifest["data_contract"]["spiral_schema_version"] == VERSIONS.storage_schema
    assert manifest["runtime"]["training_backend"]["api_id"] == VERSIONS.training_protocol.id
    assert (
        manifest["runtime"]["training_backend"]["api_version"] == VERSIONS.training_protocol.major
    )
    assert (
        manifest["runtime"]["training_backend"]["api_revision"] == VERSIONS.training_protocol.minor
    )


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


def test_real_pytorch_act_train_checkpoint_and_infer_round_trip() -> None:
    config = TrainingConfig.from_request(training_request())
    session = TrainingSession(config)
    arrays = TrainingArrays(
        owner=None,
        states=np.zeros((1, 2), dtype=np.float32),
        images=np.zeros((1, 3 * 32 * 32), dtype=np.uint8),
        actions=np.zeros((1, 4), dtype=np.float32),
        action_is_pad=np.zeros((1, 2), dtype=np.uint8),
    )
    trained = session.train_step(arrays, 1)
    assert math.isfinite(trained["loss"])
    checkpoint = session.checkpoint(
        {
            "manifest_json": "{}",
            "state_mean": [0.0, 0.0],
            "state_std": [1.0, 1.0],
            "action_mean": [0.0, 0.0],
            "action_std": [1.0, 1.0],
        }
    )
    assert len(checkpoint) > 1_000
    del session
    gc.collect()

    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app(policy_device="cpu"))
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
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
