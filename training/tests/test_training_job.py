"""A whole run: posted to the control endpoint, prepared and trained by the job on CPU, its
checkpoints served and loaded for inference."""

from __future__ import annotations

import json
import math

import numpy as np
import pytest
from conftest import AUTHORIZED, SyntheticRun, fixture

from spiral_training_runtime import act, stop
from spiral_training_runtime.act import PolicySession
from spiral_training_runtime.arrow import InferenceArrays
from spiral_training_runtime.run import TrainingJob

RUN = "/v1/runs/fixture-project/train-1791300000000-0"


class Preempted(BaseException):
    """A container going away mid-step: nothing in the job may catch it."""


@pytest.fixture
def posted(client, volume) -> str:
    """The fixture run, posted with its streams uploaded; its job's request."""
    SyntheticRun.from_fixture(volume).write()
    assert (
        client.post("/v1/runs", json=fixture("run-request.json"), headers=AUTHORIZED).status_code
        == 201
    )
    return json.dumps(fixture("run-request.json"))


def test_a_run_trains_reports_and_checkpoints(client, volume, calls, stop_flags, posted) -> None:
    TrainingJob(volume, stop_flags, posted).run()
    calls.finished.add("call-1")

    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "completed", history["message"]
    assert history["current_step"] == 6
    assert [checkpoint["step"] for checkpoint in history["checkpoints"]] == [3, 6]
    assert set(history) == set(fixture("run-history.json"))
    assert set(history["checkpoints"][0]) == set(fixture("run-history.json")["checkpoints"][0])
    metric_keys = set(fixture("run-history.json")["metrics"][0])
    assert [set(metric) for metric in history["metrics"]] == [metric_keys] * 6
    assert [metric["step"] for metric in history["metrics"]] == [1, 2, 3, 4, 5, 6]
    # Half the episodes are held out and validated every step, since checkpoints come every 3.
    assert all(metric["event"] == "validation" for metric in history["metrics"])
    assert all(math.isfinite(metric["validation_loss"]) for metric in history["metrics"])
    assert history["logs"][0] == "Prepared 2 episodes: 55 ticks of 2 cameras at 64x48."
    assert "Checkpoint 3 saved." in history["logs"]
    listed = client.get("/v1/runs/fixture-project?ids=train-1791300000000-0", headers=AUTHORIZED)
    assert set(listed.json()[0]) == set(fixture("run-status.json"))
    assert listed.json()[0]["message"] == "Completed 6 steps."


def test_a_saved_checkpoint_loads_for_inference(client, volume, stop_flags, posted) -> None:
    TrainingJob(volume, stop_flags, posted).run()

    config = client.get(f"{RUN}/checkpoints/3/config", headers=AUTHORIZED)
    model = client.get(f"{RUN}/checkpoints/3/model", headers=AUTHORIZED)
    assert config.status_code == model.status_code == 200
    assert config.json()["spiral_camera_order"] == [
        "observation.images.top_camera",
        "observation.images.left_wrist_camera",
    ]
    assert config.json()["spiral_sample_hz"] == 10.0

    policy = PolicySession.from_artifact(config.content, model.content, "cpu")

    assert policy.specification["action_chunk_size"] == 4
    assert [image["name"] for image in policy.specification["image_inputs"]] == config.json()[
        "spiral_camera_order"
    ]
    images = tuple(np.zeros((1, 48 * 64 * 3), np.uint8) for _ in range(2))
    inferred = policy.infer(InferenceArrays(None, np.zeros((1, 3), np.float32), images))
    assert len(inferred["values"]) == 4 * 3
    assert all(math.isfinite(value) for value in inferred["values"])


def test_the_checkpoint_manifest_carries_the_data_contract(volume, stop_flags, posted) -> None:
    from safetensors import safe_open

    TrainingJob(volume, stop_flags, posted).run()
    weights = (
        volume.run(SyntheticRun.from_fixture(volume).request.key).checkpoint(6)
        / "model.safetensors"
    )

    with safe_open(weights, "pt") as checkpoint:
        manifest = json.loads(checkpoint.metadata()["spiral_robotics"])

    assert manifest == {
        "format": "spiral_robotics.act.safetensors.v2",
        "run_id": "train-1791300000000-0",
        "step": 6,
        "data_contract": {
            "robot_type": "fixture_robot",
            "joint_names": ["left.j1", "left.j2", "left.gripper"],
            "camera_streams": ["/top-camera", "/left-wrist-camera"],
            "sample_hz": 10.0,
            "image_size": {"width": 64, "height": 48},
        },
    }


def test_a_stopped_run_saves_a_final_checkpoint(
    client, volume, stop_flags, posted, monkeypatch
) -> None:
    monkeypatch.setattr(stop, "STOP_POLL_SECONDS", 0.0)

    class StopAtStepThree(dict):
        reads = 0

        def get(self, flag: str, default: bool = False) -> bool:
            self.reads += 1
            return self.reads >= 3

    TrainingJob(volume, StopAtStepThree(), posted).run()

    status = client.get(RUN, headers=AUTHORIZED).json()
    assert status["phase"] == "stopped"
    assert status["current_step"] == 2
    assert [checkpoint["step"] for checkpoint in status["checkpoints"]] == [2]


def test_a_retried_job_resumes_from_its_last_checkpoint(
    client, volume, stop_flags, posted, monkeypatch
) -> None:
    train_step = act.TrainingSession.train_step

    def preempted_at_step_five(session, batch, step):
        if step == 5:
            raise Preempted
        return train_step(session, batch, step)

    monkeypatch.setattr(act.TrainingSession, "train_step", preempted_at_step_five)
    with pytest.raises(Preempted):
        TrainingJob(volume, stop_flags, posted).run()
    assert client.get(RUN, headers=AUTHORIZED).json()["phase"] == "running"

    monkeypatch.setattr(act.TrainingSession, "train_step", train_step)
    TrainingJob(volume, stop_flags, posted).run()

    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "completed"
    assert [metric["step"] for metric in history["metrics"]] == [1, 2, 3, 4, 5, 6]
    assert [checkpoint["step"] for checkpoint in history["checkpoints"]] == [3, 6]
    assert "Resumed from checkpoint 3." in history["logs"]


def test_a_finished_run_is_not_trained_again(client, volume, stop_flags, posted) -> None:
    TrainingJob(volume, stop_flags, posted).run()
    before = client.get(RUN, headers=AUTHORIZED).json()

    TrainingJob(volume, stop_flags, posted).run()

    assert client.get(RUN, headers=AUTHORIZED).json() == before


def test_a_job_that_cannot_read_its_streams_fails_with_why(
    client, volume, stop_flags, posted
) -> None:
    request = fixture("run-request.json")
    volume.stream(request["episodes"][1]["state"][0]).unlink()

    TrainingJob(volume, stop_flags, posted).run()

    status = client.get(RUN, headers=AUTHORIZED).json()
    assert status["phase"] == "failed"
    assert status["message"].startswith("episode 01a0f9c7-0000-7000-8000-000000000002:")
