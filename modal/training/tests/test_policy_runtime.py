"""The local policy runtime the robot connector calls during rollout."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pyarrow as pa
import pytest
import torch
from fastapi.testclient import TestClient

from spiral_training_runtime.act import (
    IMAGENET_MEAN,
    IMAGENET_STD,
    TrainingSession,
    _normalize_hwc_images,
)
from spiral_training_runtime.arrow import decode_inference_input
from spiral_training_runtime.config import PolicyConfig
from spiral_training_runtime.device import TrainingDevice
from spiral_training_runtime.request import RunRequest
from spiral_training_runtime.samples import Normalization
from spiral_training_runtime.service import (
    ARROW_STREAM_CONTENT_TYPE,
    POLICY_ARTIFACT_CONTENT_TYPE,
    create_app,
)
from spiral_training_runtime.testing import fixture


@pytest.fixture
def checkpoint(tmp_path: Path) -> Path:
    """An untrained checkpoint of the fixture run, written as a job writes one."""
    request = RunRequest.model_validate(fixture("run-request.json")).root
    zeros, ones = np.zeros(3, np.float32), np.ones(3, np.float32)
    normalization = Normalization(zeros, ones, zeros, ones)
    TrainingSession(request, normalization, TrainingDevice(torch.device("cpu"))).save_checkpoint(
        tmp_path, 1
    )
    return tmp_path


def fixed_list(values: np.ndarray) -> pa.FixedSizeListArray:
    return pa.FixedSizeListArray.from_arrays(pa.array(values.reshape(-1)), values.shape[1])


def inference_body(image_bytes: int) -> bytes:
    columns = {
        "state": fixed_list(np.zeros((1, 3), np.float32)),
        "image_0": fixed_list(np.zeros((1, image_bytes), np.uint8)),
        "image_1": fixed_list(np.zeros((1, image_bytes), np.uint8)),
    }
    batch = pa.RecordBatch.from_arrays(list(columns.values()), names=list(columns))
    sink = pa.BufferOutputStream()
    options = pa.ipc.IpcWriteOptions(compression="zstd")
    with pa.ipc.new_stream(sink, batch.schema, options=options) as writer:
        writer.write_batch(batch)
    return sink.getvalue().to_pybytes()


def test_arrow_values_are_zero_copy_numpy_views(checkpoint: Path) -> None:
    config = PolicyConfig.from_json(json.loads((checkpoint / "config.json").read_text()))
    body = inference_body(3 * 48 * 64)

    arrays = decode_inference_input(body, config)

    assert len(body) < 3 * 48 * 64
    assert arrays.state.shape == (1, 3)
    assert arrays.images[1].shape == (1, 3 * 48 * 64)
    assert not arrays.state.flags["OWNDATA"]
    assert not arrays.images[0].flags["OWNDATA"]


def test_a_training_checkpoint_serves_rollout_inference(checkpoint: Path) -> None:
    client = TestClient(create_app(policy_device="cpu"))
    config = (checkpoint / "config.json").read_bytes()
    artifact = (
        len(config).to_bytes(8, "little") + config + (checkpoint / "model.safetensors").read_bytes()
    )

    created = client.post(
        "/v1/policies", content=artifact, headers={"content-type": POLICY_ARTIFACT_CONTENT_TYPE}
    )
    assert created.status_code == 201, created.text
    policy_id = created.json()["session_id"]
    assert created.json()["specification"]["kind"] == "act"
    assert client.post(f"/v1/policies/{policy_id}/warm-up").status_code == 200

    inferred = client.post(
        f"/v1/policies/{policy_id}/infer",
        content=inference_body(3 * 48 * 64),
        headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
    )
    assert inferred.status_code == 200, inferred.text
    result = inferred.json()
    assert (result["steps"], result["action_dimensions"]) == (4, 3)
    assert all(math.isfinite(value) for value in result["values"])
    assert client.get("/v1/health").status_code == 200


def test_missing_nvml_never_fails_gpu_telemetry(monkeypatch: pytest.MonkeyPatch) -> None:
    def missing_nvml(_device: torch.device) -> int:
        raise ModuleNotFoundError("pynvml is unavailable")

    monkeypatch.setattr(torch.cuda, "utilization", missing_nvml)
    assert TrainingDevice(torch.device("cuda:0")).telemetry()["gpu_percent"] is None


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
