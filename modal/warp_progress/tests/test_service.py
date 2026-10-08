"""The service end to end on CPU, with a stand-in backbone and stand-in heads: the released head, and
progress models published to a local directory standing in for their Volume."""

import hashlib
import json
from pathlib import Path

import numpy as np
import pyarrow as pa
import pytest
import torch
from fastapi.testclient import TestClient
from httpx import Response
from safetensors.torch import save as save_safetensors
from video_service.service import ARROW_STREAM
from video_service.testing import AUTHORIZED, SyntheticEpisode

from warp_progress.artifacts import SELECTOR, Artifacts
from warp_progress.published import PublishedModels
from warp_progress.service import IMPLEMENTATION, REVISION, ProgressService
from warp_progress.testing import LocalModelsVolume, StandInBackbone, StandInHead
from warp_progress.warp import OUTPUT_SCHEMA, WarpModel

CONTRACT = {"codec": "h264", "framing": "annex_b", "implementation": REVISION}
FIXTURES = Path(__file__).resolve().parents[3] / "crates" / "backend" / "src" / "training" / "fixtures"
# A progress checkpoint's manifest as training writes it, and as Studio's fixture fixes it.
MANIFEST = json.loads((FIXTURES / "model-publish.json").read_text())["manifest"]


@pytest.fixture(scope="module")
def artifacts(tmp_path_factory: pytest.TempPathFactory) -> Artifacts:
    artifacts = Artifacts(tmp_path_factory.mktemp("artifacts"))
    StandInBackbone.save(artifacts.backbone)
    artifacts.head.parent.mkdir(parents=True)
    torch.save({"model": StandInHead.tensors(seed=1)}, artifacts.head)
    return artifacts


@pytest.fixture(scope="module")
def models(tmp_path_factory: pytest.TempPathFactory) -> LocalModelsVolume:
    return LocalModelsVolume(tmp_path_factory.mktemp("models"))


@pytest.fixture(scope="module")
def client(artifacts: Artifacts, models: LocalModelsVolume) -> TestClient:
    """One container's service, its heads kept across the module's tests: each test asks for models
    of its own."""
    model = WarpModel(artifacts, PublishedModels(models.root, models), torch.device("cpu"))
    return TestClient(ProgressService(model, "secret").app())


def score(client: TestClient, episode: SyntheticEpisode, model: str | None) -> Response:
    body = episode.request(CONTRACT | {"model": model})
    return client.post("/score_progress", content=body, headers=AUTHORIZED)


def scores(response: Response) -> pa.Table:
    assert response.status_code == 200, response.text
    return pa.ipc.open_stream(response.content).read_all()


def test_health_names_the_revision(client):
    health = client.get("/health").json()
    assert health == {"model": None, "implementation": IMPLEMENTATION, "revision": "warp-progress-0.2.0-fp16"}


def test_the_released_head_scores_one_row_per_picture_in_display_order(client, models, episode):
    reloads = models.reloads
    response = score(client, episode, SELECTOR)
    assert response.headers["content-type"] == ARROW_STREAM
    rows = scores(response)
    assert rows.schema == OUTPUT_SCHEMA
    assert rows["frame_index"].to_pylist() == list(range(40))
    assert rows["timestamp_ns"].to_pylist() == sorted(episode.times_ns)
    assert np.isfinite(rows["velocity"].to_numpy()).all()
    assert models.reloads == reloads


def test_a_published_head_scores_as_the_released_head_scores_with_its_weights(
    client, artifacts, models, episode
):
    released = torch.load(artifacts.head, weights_only=True)["model"]
    same = models.publish(save_safetensors(released), MANIFEST)
    centers = released["rel_bin_centers"]
    doubled = models.publish(save_safetensors(released | {"rel_bin_centers": 2 * centers}), MANIFEST)

    by_released = scores(score(client, episode, SELECTOR))
    assert scores(score(client, episode, same)).equals(by_released)
    by_doubled = scores(score(client, episode, doubled))
    assert np.abs(by_released["velocity"].to_numpy()).max() > 0
    np.testing.assert_array_equal(by_doubled["velocity"].to_numpy(), 2 * by_released["velocity"].to_numpy())
    np.testing.assert_allclose(by_doubled["progress"].to_numpy(), by_released["progress"].to_numpy(), atol=1e-6)


def test_a_model_published_after_a_request_for_it_is_found_and_then_kept(client, models, episode):
    weights = save_safetensors(StandInHead.tensors(seed=2))
    model = f"spiral-robotics-models/{hashlib.sha256(weights).hexdigest()}"
    reloads = models.reloads

    missing = score(client, episode, model)
    assert missing.status_code == 404
    assert missing.json() == {"error": f"no progress model {model} is published"}
    assert models.publish(weights, MANIFEST) == model
    scores(score(client, episode, model))
    scores(score(client, episode, model))

    # A head not loaded yet is looked up after a reload; a loaded one is kept.
    assert models.reloads == reloads + 2


@pytest.mark.parametrize(
    "model",
    [
        None,
        "uynitsuj/warp-rm-sim-bottles-sss15@main",
        "spiral-robotics-models/" + "AB" * 32,
        "spiral-robotics-models/abc",
    ],
)
def test_a_request_naming_neither_kind_of_model_is_not_found(client, episode, model):
    response = score(client, episode, model)
    assert response.status_code == 404
    assert response.json() == {
        "error": f"unknown model {model!r}; expected {SELECTOR!r} or spiral-robotics-models/<sha256>"
    }


@pytest.mark.parametrize(
    "change",
    [
        {"family": "act"},
        {"manifest": "spiral_robotics.progress.safetensors.v2"},
        {"backbone": "facebook/dinov3-vitb16-pretrain-lvd1689m@main"},
        {"window_size": 16},
        {"feature_stride": 2},
        {"standard_stride_src": 10},
        {"fps": 15},
        {"bins": 20},
        {"bin_range": [-1.0, 1.0]},
        {"ablation": "full"},
        {"image": {"width": 256, "height": 256}},
        {"crop": "center"},
    ],
)
def test_a_published_model_made_for_another_scorer_is_a_conflict(client, models, episode, change):
    (key, value), = change.items()
    # Never read: the manifest is checked first.
    model = models.publish(f"weights made for {key} {value}".encode(), MANIFEST | change)

    response = score(client, episode, model)

    assert response.status_code == 409
    assert response.json() == {
        "error": f"published model {model} cannot be scored here: its {key} is {value!r}, "
        f"this scorer's {MANIFEST[key]!r}"
    }
