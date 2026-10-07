import numpy as np
import pyarrow as pa
import pytest
from fastapi.testclient import TestClient
from video_service.episode import EpisodeVideo
from video_service.service import ARROW_STREAM
from video_service.testing import AUTHORIZED
from video_service.timing import StageTimes

from warp_progress.artifacts import SELECTOR
from warp_progress.service import REVISION, ProgressService
from warp_progress.warp import OUTPUT_SCHEMA, DenseScores

CONTRACT = {"codec": "h264", "framing": "annex_b", "model": SELECTOR, "implementation": REVISION}


class DecodingModel:
    """Scores every decoded picture with a constant rising window, so no weights are needed."""

    def score(self, video: EpisodeVideo, times: StageTimes) -> pa.RecordBatch:
        with times.stage("decode"):
            timestamps = np.array([frame.pts for frame in video.frames()], np.uint64)
        rising = np.tile(np.arange(32, dtype=np.float32) / 31, (len(timestamps) - 31, 1))
        return DenseScores(len(timestamps), 1, rising).record_batch(timestamps)


@pytest.fixture
def client() -> TestClient:
    return TestClient(ProgressService(DecodingModel(), "secret").app())


def test_health_names_the_revision(client):
    health = client.get("/health").json()
    assert health["model"] == SELECTOR and health["implementation"]
    assert health["revision"] == "warp-progress-0.2.0-fp16"


def test_scores_one_row_per_picture_in_display_order(client, episode):
    response = client.post("/score_progress", content=episode.request(CONTRACT), headers=AUTHORIZED)
    assert response.status_code == 200 and response.headers["content-type"] == ARROW_STREAM
    scores = pa.ipc.open_stream(response.content).read_all()
    assert scores.schema == OUTPUT_SCHEMA
    assert scores["frame_index"].to_pylist() == list(range(40))
    assert scores["timestamp_ns"].to_pylist() == sorted(episode.times_ns)
