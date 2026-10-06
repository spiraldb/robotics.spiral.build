"""The wire fixtures the Rust backend's tests also read."""

from __future__ import annotations

import pytest
from conftest import fixture

from spiral_training_runtime.report import RunHistory, RunStatus
from spiral_training_runtime.request import RunRequest


def test_run_request_fixture_parses() -> None:
    request = RunRequest.model_validate(fixture("run-request.json"))

    assert request.key.flag == "fixture-project/train-1791300000000-0"
    assert request.camera_features() == [
        "observation.images.top_camera",
        "observation.images.left_wrist_camera",
    ]
    assert len(request.stream_keys()) == 12
    assert request.config.optimization.checkpoint_every == 3


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
        lambda request: request["config"]["observation"]["cameras"].append("/top_camera"),
        lambda request: request["config"].update(unexpected=True),
    ],
)
def test_run_requests_that_break_the_contract_are_rejected(change) -> None:
    request = fixture("run-request.json")
    change(request)

    with pytest.raises(ValueError):
        RunRequest.model_validate(request)
