"""A whole run: posted to the control endpoint, prepared and trained by the job on CPU, and
checkpointed to the Volume."""

from __future__ import annotations

import json
import math

import pytest
import torch

from spiral_training_runtime import act, stop
from spiral_training_runtime.act import (
    IMAGENET_MEAN,
    IMAGENET_STD,
    ActTraining,
    _normalize_hwc_images,
)
from spiral_training_runtime.device import StepResult, TrainingDevice
from spiral_training_runtime.divergence import Divergence, DivergenceWatch
from spiral_training_runtime.request import ActRunRequest
from spiral_training_runtime.run import TrainingJob
from spiral_training_runtime.testing import AUTHORIZED, SyntheticRun, fixture

RUN = "/v1/runs/fixture-project/train-1791300000000-0"


class Preempted(BaseException):
    """A container going away mid-step: nothing in the job may catch it."""


@pytest.fixture
def posted(client, volume) -> ActTraining:
    """The fixture run, posted with its streams uploaded; its job's family, as the `train`
    function builds it."""
    SyntheticRun.from_fixture(volume).write()
    assert (
        client.post("/v1/runs", json=fixture("run-request.json"), headers=AUTHORIZED).status_code
        == 201
    )
    return ActTraining(ActRunRequest.model_validate(fixture("run-request.json")), volume)


@pytest.fixture
def validation_losses(monkeypatch):
    """Scripts the validation losses a run measures, one per validation, in order."""

    def script(losses: list[float]) -> None:
        measured = iter(losses)
        monkeypatch.setattr(
            act.TrainingSession,
            "validate",
            lambda session, batch: StepResult(next(measured), 0.0, 0.0),
        )

    return script


def checkpoint_steps(history: dict) -> list[int]:
    return [checkpoint["step"] for checkpoint in history["checkpoints"]]


def best_step(history: dict) -> int:
    """The first step measuring the lowest validation loss."""
    validations = [metric for metric in history["metrics"] if metric["validation_loss"] is not None]
    return min(validations, key=lambda metric: metric["validation_loss"])["step"]


def test_a_run_trains_reports_and_checkpoints(client, volume, calls, stop_flags, posted) -> None:
    TrainingJob(volume, stop_flags, posted).run()
    calls.finished.add("call-1")

    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "completed", history["message"]
    assert history["current_step"] == 6
    # Every third step, and the one measuring the lowest validation loss.
    assert checkpoint_steps(history) == sorted({3, 6, best_step(history)})
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


def test_a_saved_checkpoint_config_names_its_cameras_and_rate(volume, stop_flags, posted) -> None:
    TrainingJob(volume, stop_flags, posted).run()
    checkpoint = volume.run(SyntheticRun.from_fixture(volume).request.key).checkpoint(3)

    assert (checkpoint / "model.safetensors").is_file()
    config = json.loads((checkpoint / "config.json").read_text())
    assert config["spiral_camera_order"] == [
        "observation.images.top_camera",
        "observation.images.left_wrist_camera",
    ]
    assert config["spiral_sample_hz"] == 10.0


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
    assert checkpoint_steps(status) == sorted({2, best_step(status)})


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
    assert checkpoint_steps(history) == sorted({3, 6, best_step(history)})
    assert "Resumed from checkpoint 3." in history["logs"]


@pytest.mark.parametrize(
    ("losses", "kept"),
    [
        # Step 5's loss is the lowest: kept beside the regular checkpoints, in place of step 2's.
        ([2.0, 1.0, 1.5, 1.2, 0.9, 1.1], [3, 5, 6]),
        # Step 3's is, and its regular checkpoint holds it: step 2's goes.
        ([2.0, 1.0, 0.5, 1.2, 0.9, 1.1], [3, 6]),
    ],
)
def test_the_checkpoint_with_the_lowest_validation_loss_is_kept(
    client, volume, stop_flags, posted, validation_losses, losses, kept
) -> None:
    validation_losses(losses)

    TrainingJob(volume, stop_flags, posted).run()

    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "completed"
    assert checkpoint_steps(history) == kept
    checkpoints = volume.run(posted.request.key).checkpoint(0).parent
    assert sorted(int(checkpoint.name) for checkpoint in checkpoints.iterdir()) == kept
    if 5 in kept:
        assert "Checkpoint 5 saved: the lowest validation loss yet." in history["logs"]


def test_a_diverged_run_fails_at_once_keeping_its_checkpoints(
    client, volume, stop_flags, posted, validation_losses, monkeypatch
) -> None:
    validation_losses([2.0, 1.0, 1.5, 1.2, 1.3, 1.4])

    def diverged_at_step_four(watch, step: int, loss: float) -> Divergence | None:
        return Divergence(step, 0.52, 2.9) if step == 4 else None

    monkeypatch.setattr(DivergenceWatch, "observe", diverged_at_step_four)

    TrainingJob(volume, stop_flags, posted).run()

    history = client.get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "failed"
    assert history["current_step"] == 4
    assert history["message"] == (
        "Diverged at step 4: the training loss rose from 0.52 to 2.90 and stayed there. "
        "Checkpoint 2 has the lowest validation loss."
    )
    assert history["message"] in history["logs"]
    # No final checkpoint of the diverged weights.
    assert checkpoint_steps(history) == [2, 3]


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
