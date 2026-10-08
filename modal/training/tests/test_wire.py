"""The wire fixtures the Rust backend's tests also read."""

from __future__ import annotations

import pytest

from spiral_training_runtime.report import RunHistory, RunStatus
from spiral_training_runtime.request import ActRunRequest, ProgressRunRequest, RunRequest
from spiral_training_runtime.testing import fixture


def test_run_request_fixture_parses_as_act() -> None:
    request = RunRequest.model_validate(fixture("run-request.json")).root

    assert isinstance(request, ActRunRequest)
    assert request.key.flag == "fixture-project/train-1791300000000-0"
    assert request.camera_features() == [
        "observation.images.top_camera",
        "observation.images.left_wrist_camera",
    ]
    assert len(request.stream_keys()) == 12
    assert request.config.model.chunk_size == 4
    assert request.config.optimization.checkpoint_every == 3


def test_progress_run_request_fixture_parses_as_progress() -> None:
    request = RunRequest.model_validate(fixture("progress-run-request.json")).root

    assert isinstance(request, ProgressRunRequest)
    assert request.key.flag == "fixture-project/train-1791300000000-1"
    assert request.config.model.camera == "/top-camera"
    assert request.stream_keys() == ["1" * 64, "7" * 64]
    assert [len(episode.intervals) for episode in request.episodes] == [1, 2]
    assert request.config.optimization.batch_size == 4


@pytest.mark.parametrize(
    ("name", "model"), [("run-status.json", RunStatus), ("run-history.json", RunHistory)]
)
def test_status_and_history_fixtures_round_trip(name: str, model: type[RunStatus]) -> None:
    expected = fixture(name)

    assert model.model_validate(expected).model_dump(mode="json") == expected


def test_health_matches_its_fixture(client) -> None:
    response = client.get("/v1/health")

    assert response.status_code == 200
    assert response.json() == fixture("health.json")


@pytest.mark.parametrize(
    "change",
    [
        lambda request: request["episodes"][0]["cameras"].pop(),
        lambda request: request["episodes"][1]["intervals"].append([9_000_000_000, 9_000_000_000]),
        lambda request: request.update(run_id="../escape"),
        lambda request: request["config"]["model"]["observation"]["cameras"].append("/top_camera"),
        lambda request: request["config"].update(unexpected=True),
        lambda request: request["config"]["optimization"].update(chunk_size=4),
        lambda request: request["config"]["model"].update(kind="progress"),
        lambda request: request["config"]["model"].pop("kind"),
    ],
)
def test_act_run_requests_that_break_the_contract_are_rejected(change) -> None:
    request = fixture("run-request.json")
    change(request)

    with pytest.raises(ValueError):
        RunRequest.model_validate(request)


@pytest.mark.parametrize(
    "change",
    [
        lambda request: request["episodes"][0].pop("camera"),
        lambda request: request["episodes"][0].update(cameras=["1" * 64]),
        lambda request: request["episodes"][1]["intervals"].append(
            [40_000_000_000, 39_000_000_000]
        ),
        lambda request: request["config"]["model"].update(camera=""),
        lambda request: request["config"]["model"].update(chunk_size=4),
        lambda request: request.update(robot_type="fixture_robot"),
        lambda request: request["config"]["model"].update(kind="act"),
    ],
)
def test_progress_run_requests_that_break_the_contract_are_rejected(change) -> None:
    request = fixture("progress-run-request.json")
    change(request)

    with pytest.raises(ValueError):
        RunRequest.model_validate(request)
