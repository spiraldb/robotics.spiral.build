"""Publishing a progress model through the control endpoint, from a local training volume to a local
models volume."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from spiral_training_runtime.request import RunKey
from spiral_training_runtime.testing import (
    AUTHORIZED,
    SyntheticProgressRun,
    SyntheticRun,
    fixture,
)
from spiral_training_runtime.volume import TrainingVolume

PROGRESS_RUN = RunKey("fixture-project", "train-1791300000000-1")
ACT_RUN = RunKey("fixture-project", "train-1791300000000-0")
WEIGHTS = b"the head's tensors"


def post_run(client, volume: TrainingVolume, request: str) -> None:
    """Posts a fixture run, its streams uploaded first."""
    synthetic = SyntheticProgressRun if request == "progress-run-request.json" else SyntheticRun
    synthetic.from_fixture(volume).write()
    assert client.post("/v1/runs", json=fixture(request), headers=AUTHORIZED).status_code == 201


def write_checkpoint(volume: TrainingVolume, key: RunKey, step: int, manifest: dict) -> None:
    """A checkpoint as a run's job writes it."""
    directory = volume.run(key).checkpoint(step)
    directory.mkdir(parents=True)
    (directory / "config.json").write_text(json.dumps(manifest))
    (directory / "model.safetensors").write_bytes(WEIGHTS)


def publish(client, request: dict | None = None):
    body = request or fixture("model-publish-request.json")
    return client.post("/v1/models", json=body, headers=AUTHORIZED)


@pytest.fixture
def progress_checkpoint(client, volume: TrainingVolume) -> dict:
    """The progress fixture run, posted, with the checkpoint the publish fixture names: its
    manifest."""
    post_run(client, volume, "progress-run-request.json")
    manifest = fixture("model-publish.json")["manifest"]
    write_checkpoint(volume, PROGRESS_RUN, 3, manifest)
    return manifest


def test_publishing_copies_a_checkpoint_under_the_hash_of_its_weights(
    client, models: Path, progress_checkpoint: dict
) -> None:
    digest = hashlib.sha256(WEIGHTS).hexdigest()

    published = publish(client)

    assert published.status_code == 201
    expected = fixture("model-publish.json") | {"model": f"spiral-robotics-models/{digest}"}
    assert published.json() == expected
    copied = models / "models" / digest
    assert sorted(path.relative_to(models).as_posix() for path in models.rglob("*")) == [
        "models",
        f"models/{digest}",
        f"models/{digest}/config.json",
        f"models/{digest}/model.safetensors",
    ]
    assert (copied / "model.safetensors").read_bytes() == WEIGHTS
    assert json.loads((copied / "config.json").read_text()) == progress_checkpoint


def test_publishing_again_leaves_the_published_model_as_is(
    client, volume: TrainingVolume, models: Path, progress_checkpoint: dict
) -> None:
    first = publish(client)
    weights = models / "models" / hashlib.sha256(WEIGHTS).hexdigest() / "model.safetensors"
    copied = weights.stat()
    # Another checkpoint with the same weights is the same model.
    write_checkpoint(volume, PROGRESS_RUN, 6, progress_checkpoint | {"step": 6})

    again = publish(client)
    same_weights = publish(client, fixture("model-publish-request.json") | {"step": 6})

    assert (again.status_code, again.json()) == (200, first.json())
    assert same_weights.status_code == 200
    assert same_weights.json()["model"] == first.json()["model"]
    assert same_weights.json()["manifest"]["step"] == 6
    assert (weights.stat().st_ino, weights.stat().st_mtime_ns) == (
        copied.st_ino,
        copied.st_mtime_ns,
    )


def test_publishing_after_a_copy_that_went_away_mid_way_copies_it_whole(
    client, models: Path, progress_checkpoint: dict
) -> None:
    digest = hashlib.sha256(WEIGHTS).hexdigest()
    partial = models / "models" / f"{digest}.partial"
    partial.mkdir(parents=True)
    (partial / "model.safetensors").write_bytes(WEIGHTS[:4])

    assert publish(client).status_code == 201

    assert not partial.exists()
    assert (models / "models" / digest / "model.safetensors").read_bytes() == WEIGHTS


def test_publishing_an_unknown_run_or_step_is_not_found(
    client, models: Path, progress_checkpoint: dict
) -> None:
    request = fixture("model-publish-request.json")

    unknown_run = publish(client, request | {"run_id": "train-1791300000000-9"})
    unknown_step = publish(client, request | {"step": 4})

    assert (unknown_run.status_code, unknown_run.json()) == (
        404,
        {"error": "run train-1791300000000-9 is unknown"},
    )
    assert (unknown_step.status_code, unknown_step.json()) == (
        404,
        {"error": "run train-1791300000000-1 has no checkpoint 4"},
    )
    assert not models.exists()


def test_publishing_a_checkpoint_that_is_no_progress_model_is_a_conflict(
    client, volume: TrainingVolume, models: Path
) -> None:
    post_run(client, volume, "run-request.json")
    write_checkpoint(volume, ACT_RUN, 3, {"type": "act", "spiral_sample_hz": 15.0})

    response = publish(client, {"project": "fixture-project", "run_id": ACT_RUN.run_id, "step": 3})

    assert response.status_code == 409
    assert response.json() == {
        "error": "checkpoint 3 of run train-1791300000000-0 is not a progress model; only those "
        "are published"
    }
    assert not models.exists()


def test_a_malformed_publish_request_answers_why(client) -> None:
    response = publish(client, {"project": "fixture-project", "run_id": "../escape", "step": 3})

    assert response.status_code == 422
    assert "run_id" in response.json()["error"]
