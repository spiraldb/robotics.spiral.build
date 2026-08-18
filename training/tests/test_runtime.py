from __future__ import annotations

import asyncio
import gc
import json
import math
import threading
import time
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
from spiral_training_runtime.arrow import decode_training_batch
from spiral_training_runtime.config import TrainingConfig
from spiral_training_runtime.errors import RuntimeRequestError
from spiral_training_runtime.jobs import (
    MAX_LOG_HISTORY,
    MAX_METRIC_HISTORY,
    TrainingJob,
    _checkpoint_manifest,
)
from spiral_training_runtime.models import (
    TrainingBackendInfoModel,
    TrainingJobRequestModel,
    TrainingJobSnapshotModel,
)
from spiral_training_runtime.protocol import BOUNDARIES
from spiral_training_runtime.service import (
    ARROW_STREAM_CONTENT_TYPE,
    POLICY_ARTIFACT_CONTENT_TYPE,
    create_app,
)
from spiral_training_runtime.spiral import (
    Anchor,
    Sample,
    SpiralActDataset,
    VideoSegment,
    connect_spiral,
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
        "spiral_schema_version": BOUNDARIES.spiral_schema,
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


@pytest.mark.parametrize(
    ("fixture", "model"),
    [
        ("health.json", TrainingBackendInfoModel),
        ("job-request.json", TrainingJobRequestModel),
        ("job-snapshot.json", TrainingJobSnapshotModel),
    ],
)
def test_canonical_wire_fixtures_round_trip_through_python_models(
    fixture: str, model: type
) -> None:
    source = (Path(__file__).resolve().parents[1] / "contracts" / "fixtures" / fixture).read_text(
        encoding="utf-8"
    )
    expected = json.loads(source)

    actual = model.model_validate(expected).model_dump(mode="json")

    assert actual == expected


def test_canonical_fixture_versions_follow_the_boundary_descriptor() -> None:
    fixtures = Path(__file__).resolve().parents[1] / "contracts" / "fixtures"
    health = json.loads((fixtures / "health.json").read_text(encoding="utf-8"))
    request = json.loads((fixtures / "job-request.json").read_text(encoding="utf-8"))

    assert health["api_id"] == BOUNDARIES.training_api.id
    assert health["api_version"] == BOUNDARIES.training_api.major
    assert health["api_revision"] == BOUNDARIES.training_api.minor
    assert health["spiral_schema_minimum"] == BOUNDARIES.training_api.spiral_schema.minimum
    assert health["spiral_schema_maximum"] == BOUNDARIES.training_api.spiral_schema.maximum
    assert request["spiral_schema_version"] == BOUNDARIES.spiral_schema


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


def test_missing_managed_video_relation_falls_back_to_raw_spiral_payload(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dataset = object.__new__(SpiralActDataset)
    dataset.namespace = "spiral_robotics"
    dataset.camera_streams = ("observation.images.wrist",)
    dataset.image_width = 2
    dataset.image_height = 1
    dataset.resize_images = False
    dataset.video_decoder = "auto"
    sample = Sample(
        episode_id="episode",
        stream_id="follower",
        sample_index=0,
        episode_time_ns=0,
        sample_hz=30.0,
        state=np.array([0], dtype=np.float32),
        action=np.array([0], dtype=np.float32),
        robot_type="so101_follower",
        joint_names=("joint",),
    )
    segment = VideoSegment(
        episode_id="episode",
        stream_id="observation.images.wrist",
        segment_index=0,
        timeline_offset_ns=0,
        fps=30,
        frames=1,
        width=2,
        height=1,
        media_relation="spiral_robotics.video_episode.assets",
    )
    dataset.segments = [segment]
    queries: list[str] = []

    def query(sql: str) -> pa.Table:
        queries.append(sql)
        if "SELECT payload FROM spiral_robotics.video_assets" in sql:
            return pa.table({"payload": [b"mp4"]})
        raise RuntimeRequestError(
            "Spiral training query failed: unknown table `spiral_robotics.video_episode.assets`",
            422,
        )

    monkeypatch.setattr(dataset, "_query", query)
    monkeypatch.setattr(
        "spiral_training_runtime.spiral._decode_mp4_frames",
        lambda payload, frames, width, height, resize: {
            0: np.full((height, width, 3), 17, dtype=np.uint8)
        },
    )

    images = dataset._fetch_images([Anchor(sample, 0, 0)])

    assert images.shape == (1, 1, 1, 2, 3)
    assert images.tolist() == [[[[[17, 17, 17], [17, 17, 17]]]]]
    assert any("video_episode.assets" in query for query in queries)
    assert any("SELECT payload FROM spiral_robotics.video_assets" in query for query in queries)


def test_spiral_image_query_casts_request_index_to_arrow_integer(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dataset = object.__new__(SpiralActDataset)
    dataset.camera_streams = ("observation.images.wrist",)
    dataset.image_width = 2
    dataset.image_height = 1
    dataset.resize_images = False
    dataset.video_decoder = "auto"
    sample = Sample(
        episode_id="episode",
        stream_id="follower",
        sample_index=0,
        episode_time_ns=0,
        sample_hz=30.0,
        state=np.array([0], dtype=np.float32),
        action=np.array([0], dtype=np.float32),
        robot_type="so101_follower",
        joint_names=("joint",),
    )
    dataset.segments = [
        VideoSegment(
            episode_id="episode",
            stream_id="observation.images.wrist",
            segment_index=0,
            timeline_offset_ns=0,
            fps=30,
            frames=1,
            width=2,
            height=1,
            media_relation="spiral_robotics.video_episode.assets",
        )
    ]
    queries: list[str] = []

    def query(sql: str) -> pa.Table:
        queries.append(sql)
        return pa.table(
            {
                "group_index": pa.array([0], type=pa.int64()),
                "request_index": pa.array([0], type=pa.int64()),
                "image": [
                    {
                        "data": [1, 2, 3, 4, 5, 6],
                        "shape": [1, 2, 3],
                    }
                ],
            }
        )

    monkeypatch.setattr(dataset, "_query", query)

    images = dataset._fetch_images([Anchor(sample, 0, 0)])

    assert images.tolist() == [[[[[1, 2, 3], [4, 5, 6]]]]]
    assert "CAST(f.request_index AS BIGINT) AS request_index" in queries[0]


def test_spiral_batches_all_video_segments_into_one_query(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dataset = object.__new__(SpiralActDataset)
    dataset.camera_streams = ("observation.images.wrist",)
    dataset.image_width = 1
    dataset.image_height = 1
    dataset.resize_images = False
    dataset.video_decoder = "auto"
    samples = [
        Sample(
            episode_id=episode,
            stream_id="follower",
            sample_index=0,
            episode_time_ns=0,
            sample_hz=30.0,
            state=np.array([0], dtype=np.float32),
            action=np.array([0], dtype=np.float32),
            robot_type="so101_follower",
            joint_names=("joint",),
        )
        for episode in ("episode-a", "episode-b")
    ]
    dataset.segments = [
        VideoSegment(
            episode_id=sample.episode_id,
            stream_id="observation.images.wrist",
            segment_index=0,
            timeline_offset_ns=0,
            fps=30,
            frames=1,
            width=1,
            height=1,
            media_relation="spiral_robotics.video_episode.assets",
        )
        for sample in samples
    ]
    queries: list[str] = []

    def query(sql: str) -> pa.Table:
        queries.append(sql)
        return pa.table(
            {
                "group_index": pa.array([0, 1], type=pa.int64()),
                "request_index": pa.array([0, 0], type=pa.int64()),
                "image": [
                    {"data": [1, 2, 3], "shape": [1, 1, 3]},
                    {"data": [4, 5, 6], "shape": [1, 1, 3]},
                ],
            }
        )

    monkeypatch.setattr(dataset, "_query", query)

    images = dataset._fetch_images([Anchor(sample, 0, 0) for sample in samples])

    assert len(queries) == 1
    assert " UNION ALL " in queries[0]
    assert "0 AS group_index" in queries[0]
    assert "1 AS group_index" in queries[0]
    assert images.tolist() == [
        [[[[1, 2, 3]]]],
        [[[[4, 5, 6]]]],
    ]


def test_training_batch_stays_within_one_weighted_episode() -> None:
    dataset = object.__new__(SpiralActDataset)
    dataset.batch_size = 16
    population = [
        Anchor(
            Sample(
                episode_id=episode,
                stream_id="follower",
                sample_index=index,
                episode_time_ns=index,
                sample_hz=30.0,
                state=np.array([index], dtype=np.float32),
                action=np.array([index], dtype=np.float32),
                robot_type="so101_follower",
                joint_names=("joint",),
            ),
            frame_index=index,
            episode_time_ns=index,
        )
        for episode, count in (("short", 2), ("long", 8))
        for index in range(count)
    ]
    rng = np.random.default_rng(42)

    batches = [dataset._select_episode_batch(population, rng) for _ in range(200)]

    assert all(len({anchor.sample.episode_id for anchor in batch}) == 1 for batch in batches)
    long_batches = sum(batch[0].sample.episode_id == "long" for batch in batches)
    assert 140 <= long_batches <= 180


def test_checkpoint_ingest_keeps_namespace_in_the_table_path() -> None:
    calls: list[tuple[str, pa.Table, dict[str, object]]] = []

    class Cursor:
        def __enter__(self) -> Cursor:
            return self

        def __exit__(self, *_args: object) -> None:
            pass

        def adbc_ingest(self, table_name: str, table: pa.Table, **options: object) -> None:
            calls.append((table_name, table, options))

    class Connection:
        def cursor(self) -> Cursor:
            return Cursor()

    dataset = object.__new__(SpiralActDataset)
    dataset.namespace = "spiral_robotics"
    dataset.connection = Connection()

    dataset.write_checkpoint(
        "run-id",
        "run-name",
        1,
        123,
        0.25,
        "{}",
        b"checkpoint",
    )

    assert len(calls) == 1
    table_name, table, options = calls[0]
    assert table_name == "spiral_robotics.training_checkpoints"
    assert table.num_rows == 1
    assert options == {"mode": "create_append"}


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
            cursor = snapshot["cursor"]
            delta = await client.get(
                "/v1/jobs/train-test",
                params={
                    "after_metric_step": cursor["metric_step"],
                    "after_checkpoint_step": cursor["checkpoint_step"],
                    "after_log_offset": cursor["log_offset"],
                },
            )
            assert delta.status_code == 200
            assert delta.json()["metrics"] == []
            assert delta.json()["checkpoints"] == []
            assert delta.json()["logs"] == []

            invalid = job_request()
            invalid["unexpected"] = True
            rejected = await client.post("/v1/jobs", json=invalid)
            assert rejected.status_code == 422

            incompatible = job_request()
            incompatible["spiral_schema_version"] = (
                BOUNDARIES.training_api.spiral_schema.maximum + 1
            )
            rejected = await client.post("/v1/jobs", json=incompatible)
            assert rejected.status_code == 422

    asyncio.run(exercise())
    assert len(writes) == 1


def test_job_replay_state_is_bounded_and_reports_compaction() -> None:
    job = TrainingJob(job_request())
    with job.lock:
        for step in range(1, MAX_METRIC_HISTORY + 3):
            job._append_metric_locked({"step": step})
        for index in range(MAX_LOG_HISTORY + 2):
            job._append_log_locked(f"line {index}")

    snapshot = job.snapshot()

    assert len(snapshot["metrics"]) == MAX_METRIC_HISTORY
    assert len(snapshot["logs"]) == MAX_LOG_HISTORY
    assert "no longer retained" in snapshot["history_gap"]


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
            health = response.json()
            assert health["api_id"] == BOUNDARIES.training_api.id
            assert health["api_version"] == BOUNDARIES.training_api.major
            assert health["api_revision"] == BOUNDARIES.training_api.minor
            assert health["spiral_schema_minimum"] == BOUNDARIES.training_api.spiral_schema.minimum
            assert health["spiral_schema_maximum"] == BOUNDARIES.training_api.spiral_schema.maximum
            assert health["id"] == "pytorch.lerobot"

            response = await client.get("/openapi.json")
            assert response.status_code == 200
            assert response.json()["info"]["version"] == BOUNDARIES.training_api.display_version

            response = await client.post(
                "/v1/sessions/missing/train?step=1",
            )
            assert response.status_code == 404
            assert response.json()["detail"] == "Not Found"

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
    session = SimpleNamespace(info={"device": "cpu"})

    manifest, _model = _checkpoint_manifest(request, dataset, session, 1)

    assert manifest["data_contract"]["spiral_schema_version"] == BOUNDARIES.spiral_schema
    assert manifest["runtime"]["training_backend"]["api_id"] == BOUNDARIES.training_api.id
    assert manifest["runtime"]["training_backend"]["api_version"] == BOUNDARIES.training_api.major
    assert manifest["runtime"]["training_backend"]["api_revision"] == BOUNDARIES.training_api.minor


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
    config = TrainingConfig.from_request(training_request())
    session = TrainingSession(config)
    session.buffer_batch(decode_training_batch(training_body(), config), "training", 1)
    trained = session.train_buffered(1)
    assert math.isfinite(trained["loss"]["total"])
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
