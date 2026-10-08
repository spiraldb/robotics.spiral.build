"""The control endpoint, against a local volume and fake function calls."""

from __future__ import annotations

import json

import numpy as np
import pytest

from spiral_training_runtime.report import RunStatus
from spiral_training_runtime.request import RunKey
from spiral_training_runtime.testing import (
    AUTHORIZED,
    SyntheticProgressRun,
    SyntheticRun,
    arrow_stream,
    fixture,
    value_table,
)

KEY = "ab" * 32
RUN = "/v1/runs/fixture-project/train-1791300000000-0"
RUN_KEY = RunKey("fixture-project", "train-1791300000000-0")


@pytest.fixture
def uploaded(volume) -> None:
    SyntheticRun.from_fixture(volume).write()


def post_run(client, request: dict | None = None):
    return client.post("/v1/runs", json=request or fixture("run-request.json"), headers=AUTHORIZED)


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("post", "/v1/streams/missing"),
        ("put", f"/v1/streams/{KEY}"),
        ("post", "/v1/runs"),
        ("get", "/v1/runs/fixture-project?ids=a"),
        ("get", RUN),
        ("post", f"{RUN}/stop"),
        ("delete", RUN),
        ("get", f"{RUN}/checkpoints/3/model"),
    ],
)
@pytest.mark.parametrize("authorization", [None, "Bearer wrong", "secret"])
def test_everything_but_health_needs_the_token(
    client, method: str, path: str, authorization
) -> None:
    headers = {} if authorization is None else {"Authorization": authorization}

    response = client.request(method, path, headers=headers)

    assert response.status_code == 401
    assert response.json() == {"error": "missing or invalid bearer token"}


def test_uploads_fill_in_missing_streams(client, volume) -> None:
    other = "cd" * 32
    missing = client.post("/v1/streams/missing", json={"keys": [KEY, other]}, headers=AUTHORIZED)
    assert missing.json() == {"missing": [KEY, other]}

    body = arrow_stream(value_table(np.array([1, 2]), np.ones((2, 3))))
    stored = client.put(f"/v1/streams/{KEY}", content=body, headers=AUTHORIZED)

    assert stored.status_code == 204
    assert volume.stream(KEY).read_bytes() == body
    missing = client.post("/v1/streams/missing", json={"keys": [KEY, other]}, headers=AUTHORIZED)
    assert missing.json() == {"missing": [other]}


@pytest.mark.parametrize(
    ("key", "body"),
    [(KEY, b"not arrow"), ("../escape", b""), ("AB" * 32, b"")],
)
def test_an_upload_that_is_no_stream_is_not_stored(client, volume, key: str, body: bytes) -> None:
    response = client.put(f"/v1/streams/{key}", content=body, headers=AUTHORIZED)

    assert response.status_code in {404, 422}
    assert "error" in response.json()
    assert not volume.stream(KEY).exists()


def test_a_run_needs_its_streams(client, calls) -> None:
    response = post_run(client)

    assert response.status_code == 422
    assert "the run reads 12 streams the service lacks" in response.json()["error"]
    assert calls.spawned == []


def test_posting_a_run_spawns_its_job_once(client, calls, uploaded) -> None:
    created = post_run(client)
    assert created.status_code == 201
    assert created.json() == RunStatus.starting(RUN_KEY.run_id).model_dump()
    assert json.loads(calls.spawned[0]) == fixture("run-request.json")
    assert calls.kinds == ["act"]

    again = post_run(client)
    assert again.status_code == 200
    assert again.json() == created.json()
    assert len(calls.spawned) == 1

    different = fixture("run-request.json")
    different["config"]["run_name"] = "another"
    conflict = post_run(client, different)
    assert conflict.status_code == 409
    assert "different request" in conflict.json()["error"]


def test_a_progress_run_spawns_on_the_progress_function(client, calls, volume) -> None:
    SyntheticProgressRun.from_fixture(volume).write()

    created = post_run(client, fixture("progress-run-request.json"))

    assert created.status_code == 201
    assert calls.kinds == ["progress"]
    assert json.loads(calls.spawned[0]) == fixture("progress-run-request.json")


def test_runs_list_only_the_runs_the_service_holds(client, uploaded) -> None:
    post_run(client)

    listed = client.get(
        "/v1/runs/fixture-project?ids=train-1791300000000-0,unknown", headers=AUTHORIZED
    )

    assert listed.status_code == 200
    assert [status["run_id"] for status in listed.json()] == ["train-1791300000000-0"]
    assert client.get("/v1/runs/fixture-project/unknown", headers=AUTHORIZED).status_code == 404
    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "starting"
    assert history["metrics"] == [] and history["logs"] == []


def test_a_run_whose_call_ended_while_active_reads_failed(client, calls, volume, uploaded) -> None:
    post_run(client)
    files = volume.run(RUN_KEY)
    files.write_status(RunStatus(run_id=RUN_KEY.run_id, phase="running", current_step=2))
    calls.finished.add("call-1")

    status = client.get(RUN, headers=AUTHORIZED).json()

    assert status["phase"] == "failed"
    assert status["current_step"] == 2


def test_stopping_before_the_job_starts_cancels_it(client, calls, uploaded) -> None:
    post_run(client)

    stopped = client.post(f"{RUN}/stop", headers=AUTHORIZED)

    assert stopped.status_code == 200
    assert stopped.json()["phase"] == "stopped"
    assert calls.cancelled == ["call-1"]
    assert client.get(RUN, headers=AUTHORIZED).json()["phase"] == "stopped"


def test_stopping_a_running_job_raises_its_flag(client, volume, stop_flags, uploaded) -> None:
    post_run(client)
    volume.run(RUN_KEY).write_status(RunStatus(run_id=RUN_KEY.run_id, phase="running"))

    stopping = client.post(f"{RUN}/stop", headers=AUTHORIZED)

    assert stopping.json()["phase"] == "stopping"
    assert stop_flags == {RUN_KEY.flag: True}
    listed = client.get(f"/v1/runs/fixture-project?ids={RUN_KEY.run_id}", headers=AUTHORIZED)
    assert listed.json()[0]["phase"] == "stopping"


def test_an_active_run_cannot_be_deleted(client, volume, uploaded) -> None:
    post_run(client)

    assert client.delete(RUN, headers=AUTHORIZED).status_code == 409
    volume.run(RUN_KEY).write_status(RunStatus(run_id=RUN_KEY.run_id, phase="completed"))
    assert client.delete(RUN, headers=AUTHORIZED).status_code == 204
    assert client.get(RUN, headers=AUTHORIZED).status_code == 404
    assert client.delete(RUN, headers=AUTHORIZED).status_code == 204


def test_a_missing_checkpoint_is_not_found(client, uploaded) -> None:
    post_run(client)

    response = client.get(f"{RUN}/checkpoints/3/config", headers=AUTHORIZED)

    assert response.status_code == 404
    assert response.json() == {"error": "run train-1791300000000-0 has no checkpoint 3"}


def test_a_malformed_run_request_answers_why(client, calls) -> None:
    request = fixture("run-request.json")
    del request["joints"]

    response = post_run(client, request)

    assert response.status_code == 422
    assert response.json() == {"error": "act.joints: Field required"}
    assert calls.spawned == []


def test_a_run_request_of_no_known_family_answers_why(client, calls) -> None:
    request = fixture("run-request.json")
    request["config"]["model"]["kind"] = "diffusion"

    response = post_run(client, request)

    assert response.status_code == 422
    assert response.json() == {"error": "config.model.kind must be act or progress"}
    assert calls.spawned == []
