import pyarrow as pa
import pytest
from fastapi.testclient import TestClient

from video_service.episode import RejectedVideo
from video_service.service import ARROW_STREAM, EpisodeRequest, RetryLater, VideoContract, VideoService
from video_service.testing import AUTHORIZED
from video_service.timing import StageTimes

MODEL = "example/model@0123abcd"
REVISION = "example-0.1.0"
CONTRACT = {"codec": "h264", "framing": "annex_b", "model": MODEL, "implementation": REVISION}
SCHEMA = pa.schema([pa.field("timestamp_ns", pa.uint64(), nullable=False)])


class Answers:
    """Answers each decoded picture with its time, or fails as told."""

    def __init__(self, failure: Exception | None = None):
        self._failure = failure
        self.metadata = []

    def __call__(self, episode: EpisodeRequest, times: StageTimes) -> pa.RecordBatch:
        self.metadata.append(episode.metadata("prompt"))
        if self._failure:
            raise self._failure
        with times.stage("decode"):
            times_ns = [frame.pts for frame in episode.video.frames()]
        return pa.record_batch([pa.array(times_ns, pa.uint64())], schema=SCHEMA)


def client(answers: Answers) -> TestClient:
    service = VideoService(VideoContract(MODEL, REVISION, "example 0.1.0 on CPU"), "secret")
    return TestClient(service.app("/answer", answers))


def test_health_needs_no_token():
    health = client(Answers()).get("/health").json()
    assert health == {"model": MODEL, "implementation": "example 0.1.0 on CPU", "revision": REVISION}


def test_answers_an_episode_with_its_stages_timed(episode):
    answers = Answers()
    response = client(answers).post("/answer", content=episode.request(CONTRACT | {"prompt": "x"}), headers=AUTHORIZED)
    assert response.status_code == 200 and response.headers["content-type"] == ARROW_STREAM
    rows = pa.ipc.open_stream(response.content).read_all()
    assert rows["timestamp_ns"].to_pylist() == sorted(episode.times_ns)
    assert answers.metadata == ["x"]
    stages = [entry.split(";dur=")[0] for entry in response.headers["server-timing"].split(", ")]
    assert stages == ["request", "decode", "response"]


@pytest.mark.parametrize("authorization", [None, "Bearer wrong", "secret"])
def test_rejects_a_missing_or_wrong_token(episode, authorization):
    headers = {"Content-Type": ARROW_STREAM} | ({"Authorization": authorization} if authorization else {})
    assert client(Answers()).post("/answer", content=episode.request(CONTRACT), headers=headers).status_code == 401


@pytest.mark.parametrize("change", [{"codec": "h265"}, {"framing": "avcc"}, {"model": "example/model@main"}])
def test_rejects_another_codec_framing_or_model(episode, change):
    response = client(Answers()).post("/answer", content=episode.request(CONTRACT | change), headers=AUTHORIZED)
    assert response.status_code == 422 and "unsupported" in response.json()["error"]


@pytest.mark.parametrize("revision", [None, "example-0.0.1"])
def test_answers_a_request_for_another_revision_with_409(episode, revision):
    body = episode.request(CONTRACT | {"implementation": revision, "codec": "h265"})
    response = client(Answers()).post("/answer", content=body, headers=AUTHORIZED)
    assert response.status_code == 409
    assert response.json() == {"error": f"expected implementation revision {REVISION!r}, received {revision!r}"}


def test_rejects_a_body_that_is_not_an_arrow_stream_or_out_of_decode_order(episode):
    service = client(Answers())
    assert service.post("/answer", content=b"not arrow", headers=AUTHORIZED).status_code == 422
    table = episode.packets(CONTRACT)
    reversed_order = pa.array(range(len(table) - 1, -1, -1), pa.int64())
    body = episode.body(table.set_column(1, "decode_index", reversed_order))
    assert service.post("/answer", content=body, headers=AUTHORIZED).status_code == 422


@pytest.mark.parametrize(
    ("failure", "status"),
    [(RejectedVideo("too short"), 422), (RetryLater("CUDA out of memory"), 503)],
)
def test_reports_a_rejected_video_as_422_and_a_service_failure_as_503(episode, failure, status):
    response = client(Answers(failure)).post("/answer", content=episode.request(CONTRACT), headers=AUTHORIZED)
    assert response.status_code == status and response.json() == {"error": str(failure)}
