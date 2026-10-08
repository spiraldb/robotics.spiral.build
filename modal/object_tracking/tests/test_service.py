import pyarrow as pa
import pytest
from fastapi.testclient import TestClient
from video_service.service import ARROW_STREAM
from video_service.testing import AUTHORIZED

from object_tracking.artifacts import SELECTOR
from object_tracking.service import REVISION, TrackingService
from object_tracking.tracker import OBJECT, OUTPUT_SCHEMA, TrackingFailed
from object_tracking.video import SampledPicture

CONTRACT = {
    "codec": "h264",
    "framing": "annex_b",
    "model": SELECTOR,
    "implementation": REVISION,
    "prompt": "bottle",
    "rate": "5",
}


class BoxingTracker:
    """Tracks one object per picture, boxed where the picture's index puts it, and remembers what
    it was asked to track."""

    def __init__(self):
        self.prompts = []

    def track(self, pictures: list[SampledPicture], prompt: str) -> pa.RecordBatch:
        self.prompts.append(prompt)
        objects = [
            [{"id": 0, "score": 0.9, "x": index / 10, "y": 0.2, "width": 0.1, "height": 0.3}]
            for index, _ in enumerate(pictures)
        ]
        return pa.record_batch(
            [pa.array([picture.time_ns for picture in pictures], pa.uint64()), pa.array(objects, pa.list_(OBJECT))],
            schema=OUTPUT_SCHEMA,
        )


class FailingTracker:
    def track(self, pictures: list[SampledPicture], prompt: str) -> pa.RecordBatch:
        raise TrackingFailed("OutOfMemoryError: CUDA out of memory")


@pytest.fixture
def tracker() -> BoxingTracker:
    return BoxingTracker()


@pytest.fixture
def client(tracker) -> TestClient:
    return TestClient(TrackingService(tracker, "secret").app())


def test_health_needs_no_token(client):
    assert client.get("/health").json()["revision"] == "object-tracking-0.2.0"


def test_answers_each_sampled_picture_with_its_tracked_objects(client, tracker, episode):
    body = episode.request(CONTRACT | {"prompt": "  bottle "})
    response = client.post("/track_objects", content=body, headers=AUTHORIZED)
    assert response.status_code == 200 and response.headers["content-type"] == ARROW_STREAM
    tracks = pa.ipc.open_stream(response.content).read_all()
    assert tracks.schema == OUTPUT_SCHEMA
    assert tracks["timestamp_ns"].to_pylist()[:2] == [0, 200_000_000]
    assert tracks["objects"].to_pylist()[1] == [
        {"id": 0, "score": pytest.approx(0.9), "x": pytest.approx(0.1), "y": pytest.approx(0.2),
         "width": pytest.approx(0.1), "height": pytest.approx(0.3)}
    ]
    assert tracker.prompts == ["bottle"]
    stages = [entry.split(";dur=")[0] for entry in response.headers["server-timing"].split(", ")]
    assert stages == ["request", "decode", "track", "response"]


def test_samples_at_the_rate_the_request_names(client, episode):
    body = episode.request(CONTRACT | {"rate": "2"})
    response = client.post("/track_objects", content=body, headers=AUTHORIZED)
    assert response.status_code == 200
    tracks = pa.ipc.open_stream(response.content).read_all()
    assert tracks["timestamp_ns"].to_pylist() == [0, 500_000_000, 1_000_000_000]


@pytest.mark.parametrize(
    "change",
    [
        {"model": "facebook/sam3.1@main"},
        {"prompt": None},
        {"prompt": " "},
        {"prompt": "x" * 201},
        {"rate": None},
        {"rate": "0"},
        {"rate": "31"},
        {"rate": "fast"},
    ],
)
def test_rejects_another_model_a_missing_or_long_prompt_and_a_bad_rate(client, episode, change):
    response = client.post("/track_objects", content=episode.request(CONTRACT | change), headers=AUTHORIZED)
    assert response.status_code == 422


def test_reports_a_tracker_failure_as_a_service_error_to_ask_again(episode):
    client = TestClient(TrackingService(FailingTracker(), "secret").app())
    response = client.post("/track_objects", content=episode.request(CONTRACT), headers=AUTHORIZED)
    assert response.status_code == 503
