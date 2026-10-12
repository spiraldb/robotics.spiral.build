"""A progress model run: its camera embedded as Score progress embeds it, WARP-RM's head trained by
the sim recipe on CPU with windows drawn in worker processes, and its checkpoint published and
scored by Score progress's own `TemporalModel`. A small stand-in takes the place of the gated
DINOv3 ViT-B/16."""

from __future__ import annotations

import json
import math
import sys
from contextlib import closing
from pathlib import Path

import numpy as np
import pytest
import torch
from fastapi.testclient import TestClient
from video_service.episode import EpisodeVideo
from video_service.timing import StageTimes
from warp_progress.published import PublishedModels
from warp_progress.testing import LocalModelsVolume
from warp_progress.warp import PictureEncoder, TemporalModel, WindowPlan

from spiral_training_runtime import progress
from spiral_training_runtime.control import ControlService, RunControl
from spiral_training_runtime.progress import ProgressResources, ProgressTraining
from spiral_training_runtime.progress_embeddings import CameraEmbeddings
from spiral_training_runtime.progress_recipe import STANDARD_WINDOW_FRAMES, SimRecipe
from spiral_training_runtime.progress_windows import ProgressWindows
from spiral_training_runtime.publish import ModelPublisher
from spiral_training_runtime.request import ProgressRunRequest, RunRequest
from spiral_training_runtime.run import TrainingJob
from spiral_training_runtime.testing import (
    AUTHORIZED,
    TOKEN,
    FakeCalls,
    LocalSync,
    SyntheticCamera,
    SyntheticProgressRun,
    fixture,
)
from spiral_training_runtime.volume import TrainingVolume

RUN = "/v1/runs/fixture-project/train-1791300000000-1"
REQUEST = "progress-run-request.json"
# The fixture's kept intervals at 30 Hz: 20 s, 18 s and 21 s.
INTERVAL_FRAMES = [600, 540, 630]
# Enough to draw windows in more than one spawned process.
SAMPLING_WORKERS = 2
# The ACT side of the package, which the progress job must never import.
ACT_MODULES = ["act", "samples", "video"]


class Preempted(BaseException):
    """A container going away mid-step: nothing in the job may catch it."""


def progress_job(volume: TrainingVolume, backbone: Path) -> TrainingJob:
    """The fixture run's job, as the `train_progress` function builds it."""
    request = ProgressRunRequest.model_validate(fixture(REQUEST))
    resources = ProgressResources(backbone, SAMPLING_WORKERS)
    return TrainingJob(volume, {}, ProgressTraining(request, volume, resources))


def control_client(volume: TrainingVolume, calls: FakeCalls, models: Path) -> TestClient:
    publisher = ModelPublisher(volume, models, LocalSync())
    return TestClient(ControlService(volume, RunControl(volume, calls, {}), publisher, TOKEN).app())


def post_and_train(volume: TrainingVolume, backbone: Path, models: Path) -> TestClient:
    """The progress fixture run posted to the control endpoint, then run by its job."""
    calls = FakeCalls()
    client = control_client(volume, calls, models)
    posted = client.post("/v1/runs", json=fixture(REQUEST), headers=AUTHORIZED)
    assert posted.status_code == 201, posted.text
    assert calls.kinds == ["progress"]
    progress_job(volume, backbone).run()
    calls.finished.add("call-1")
    return client


@pytest.fixture(scope="module")
def models(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Where the models Volume is mounted."""
    return tmp_path_factory.mktemp("models")


@pytest.fixture(scope="module")
def trained_volume(tmp_path_factory: pytest.TempPathFactory) -> TrainingVolume:
    """The Volume the progress fixture run is trained on."""
    volume = TrainingVolume(tmp_path_factory.mktemp("volume"), LocalSync())
    SyntheticProgressRun.from_fixture(volume).write()
    return volume


@pytest.fixture(scope="module")
def trained(trained_volume: TrainingVolume, backbone: Path, models: Path) -> TestClient:
    """The control endpoint over the progress fixture run, trained to its end."""
    return post_and_train(trained_volume, backbone, models)


def test_a_progress_run_trains_reports_and_checkpoints(trained: TestClient) -> None:
    history = trained.get(RUN, headers=AUTHORIZED).json()

    assert history["phase"] == "completed", history["message"]
    assert history["message"] == "Completed 6 steps."
    # Every third step, and the one measuring the lowest validation loss.
    best = min(history["metrics"], key=lambda metric: metric["validation_loss"])["step"]
    assert [checkpoint["step"] for checkpoint in history["checkpoints"]] == sorted({3, 6, best})
    assert set(history) == set(fixture("run-history.json"))
    metric_keys = set(fixture("run-history.json")["metrics"][0])
    assert [set(metric) for metric in history["metrics"]] == [metric_keys] * 6
    # One of the two episodes is held out and validated every step, since checkpoints come every 3.
    assert all(metric["event"] == "validation" for metric in history["metrics"])
    assert all(math.isfinite(metric["validation_loss"]) for metric in history["metrics"])
    assert history["logs"][0] == (
        "Embedded 2 episodes of /top-camera: 1770 pictures at 30 Hz in 3 kept intervals; "
        "skipped 0 that hold no picture."
    )


def test_a_progress_run_never_imports_acts_side(trained: TestClient) -> None:
    assert trained.get(RUN, headers=AUTHORIZED).json()["phase"] == "completed"
    imported = [name for name in ACT_MODULES if f"spiral_training_runtime.{name}" in sys.modules]
    assert imported == []
    assert "lerobot" not in sys.modules


def test_the_learning_rate_follows_upstreams_warmup(trained: TestClient) -> None:
    metrics = trained.get(RUN, headers=AUTHORIZED).json()["metrics"]

    # Upstream's first step trains at the full rate; warmup then rises from 1/1000 of it.
    expected = [4e-4] + [4e-4 * step / 1000 for step in range(1, 6)]
    np.testing.assert_allclose([metric["learning_rate"] for metric in metrics], expected)


def test_the_checkpoint_manifest_describes_how_its_inputs_were_made(
    trained: TestClient, trained_volume: TrainingVolume
) -> None:
    assert trained.get(RUN, headers=AUTHORIZED).json()["phase"] == "completed"
    key = SyntheticProgressRun.from_fixture(trained_volume).request.key
    config = trained_volume.run(key).checkpoint(6) / "config.json"

    assert json.loads(config.read_text()) == {
        "family": "progress",
        "manifest": "spiral_robotics.progress.safetensors.v1",
        "run_id": "train-1791300000000-1",
        "camera": "/top-camera",
        "backbone": (
            "facebook/dinov3-vitb16-pretrain-lvd1689m@5931719e67bbdb9737e363e781fb0c67687896bc"
        ),
        "window_size": 32,
        "feature_stride": 1,
        "standard_stride_src": 15,
        "fps": 30,
        "bins": 30,
        "bin_range": [-3.0, 3.0],
        "ablation": "no_abs",
        "image": {"width": 224, "height": 224},
        "crop": "squash",
        "step": 6,
    }


def test_score_progress_scores_frames_with_a_published_checkpoint(
    trained: TestClient, backbone: Path, models: Path
) -> None:
    request = fixture("model-publish-request.json")
    published = trained.post("/v1/models", json=request, headers=AUTHORIZED)
    assert published.status_code == 201
    # Score progress finds it under the name the endpoint answered, and accepts its manifest.
    tensors = PublishedModels(models, LocalModelsVolume(models)).tensors(published.json()["model"])
    scorer = TemporalModel.from_tensors(tensors)
    trainer = SimRecipe.model()
    trainer.load_state_dict(tensors)
    trainer.eval()
    windows = torch.randn(3, 32, 768)

    with torch.inference_mode():
        torch.testing.assert_close(scorer(windows), trainer(windows)[0])

    video = SyntheticCamera.table(0, 40, (64, 48), rate=30)
    episode = EpisodeVideo(video["payload"].to_pylist(), video["log_time_ns"].to_pylist())
    encoder = PictureEncoder(backbone, torch.device("cpu"))
    embeddings = torch.cat([batch for _, batch in encoder.encode(episode, StageTimes())])
    with torch.inference_mode():
        scores = scorer.score(embeddings)
    assert len(scores.velocity) == 40
    assert np.isfinite(scores.velocity).all()


def test_the_sampler_is_upstreams_sim_recipe() -> None:
    sampler = SimRecipe.sampler()

    # Upstream's `[path-budget]` audit for the sim recipe: band [465, 2325] feature frames.
    assert (sampler.path_min_feat, sampler.path_max_feat) == (465, 2325)
    # A standard-pace window fits from 467 pictures, where Score progress keeps its full stride.
    assert STANDARD_WINDOW_FRAMES == 467
    assert [WindowPlan(frames).stride for frames in (466, 467)] == [14, 15]


@pytest.fixture
def embeddings(volume: TrainingVolume, backbone: Path, tmp_path: Path) -> CameraEmbeddings:
    """The fixture's camera embedded, with one more kept interval after its video ends."""
    SyntheticProgressRun.from_fixture(volume).write()
    request = fixture(REQUEST)
    request["episodes"][1]["intervals"].append([45_000_000_000, 46_000_000_000])
    encoder = PictureEncoder(backbone, torch.device("cpu"))
    return CameraEmbeddings(RunRequest.model_validate(request).root, volume, encoder, tmp_path)


@pytest.fixture
def windows(embeddings: CameraEmbeddings) -> ProgressWindows:
    optimization = RunRequest.model_validate(fixture(REQUEST)).root.config.optimization
    return ProgressWindows(embeddings.camera, optimization, SAMPLING_WORKERS)


def test_intervals_without_pictures_are_skipped_and_counted(
    embeddings: CameraEmbeddings,
) -> None:
    camera = embeddings.camera
    assert embeddings.skipped == 1
    assert [interval.frames for interval in camera.intervals] == INTERVAL_FRAMES
    assert [interval.episode for interval in camera.intervals] == [0, 1, 1]
    assert camera.frames == sum(INTERVAL_FRAMES)


def test_windows_come_from_one_interval_and_start_at_zero_progress(
    embeddings: CameraEmbeddings, windows: ProgressWindows
) -> None:
    batch = windows.windows(np.array([0, 1, 2, 2]))

    assert batch.features.shape == (4, 32, 768)
    assert batch.features.dtype == torch.float32
    assert batch.labels.shape == (4, 32)
    np.testing.assert_array_equal(batch.labels[:, 0].numpy(), 0.0)
    camera = embeddings.camera
    for window, index in enumerate([0, 1, 2, 2]):
        interval = camera.intervals[index]
        rows = camera.rows(np.arange(interval.first_row, interval.first_row + interval.frames))
        assert all((rows == row).all(axis=1).any() for row in batch.features[window].numpy())
    validation = windows.validation_batch(np.random.default_rng(0), 8)
    assert validation.size == 8


def test_worker_processes_draw_from_the_training_episode(
    embeddings: CameraEmbeddings, windows: ProgressWindows
) -> None:
    camera = embeddings.camera
    train = [interval for interval in camera.intervals if interval.episode == 0]
    held_out = len(camera.intervals) - len(train)
    # Seed 7 holds out the second episode, so every window comes from the first's one interval.
    assert held_out == 2
    rows = camera.rows(np.arange(train[0].first_row, train[0].first_row + train[0].frames))

    with closing(windows.train_batches(np.random.SeedSequence([7, 1]), 4)) as batches:
        drawn = [next(batches) for _ in range(3)]

    assert [batch.size for batch in drawn] == [4, 4, 4]
    for batch in drawn:
        features = batch.features.numpy().reshape(-1, 768)
        assert all((rows == row).all(axis=1).any() for row in features)


def test_a_progress_run_needs_30_hz_video(
    volume: TrainingVolume, backbone: Path, models: Path
) -> None:
    SyntheticProgressRun.from_fixture(volume, rate=15).write()

    client = post_and_train(volume, backbone, models)

    status = client.get(RUN, headers=AUTHORIZED).json()
    assert status["phase"] == "failed"
    assert status["message"].startswith(
        "episode 01a0f9c7-0000-7000-8000-000000000001: Pinned WARP sim-bottles model requires a "
        "30 Hz video"
    )


def test_a_retried_progress_job_resumes_its_model_and_schedule(
    volume: TrainingVolume, backbone: Path, models: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    SyntheticProgressRun.from_fixture(volume).write()
    train_step = progress.ProgressSession.train_step

    def preempted_at_step_five(session, batch, step):
        if step == 5:
            raise Preempted
        return train_step(session, batch, step)

    monkeypatch.setattr(progress.ProgressSession, "train_step", preempted_at_step_five)
    with pytest.raises(Preempted):
        post_and_train(volume, backbone, models)
    monkeypatch.setattr(progress.ProgressSession, "train_step", train_step)

    progress_job(volume, backbone).run()

    history = control_client(volume, FakeCalls(), models).get(RUN, headers=AUTHORIZED).json()
    assert history["phase"] == "completed"
    assert [metric["step"] for metric in history["metrics"]] == [1, 2, 3, 4, 5, 6]
    assert "Resumed from checkpoint 3." in history["logs"]
    expected = [4e-4] + [4e-4 * step / 1000 for step in range(1, 6)]
    np.testing.assert_allclose([metric["learning_rate"] for metric in history["metrics"]], expected)
