import numpy as np
import pyarrow as pa
import pytest
from fastapi.testclient import TestClient

from warp_progress.artifacts import SELECTOR
from warp_progress.service import ARROW_STREAM, REVISION, ProgressService
from warp_progress.timing import StageTimes
from warp_progress.video import PacketVideo, RejectedVideo
from warp_progress.warp import OUTPUT_SCHEMA, DenseScores

AUTHORIZED = {"Authorization": "Bearer secret", "Content-Type": ARROW_STREAM}
CONTRACT_METADATA = {"codec": "h264", "framing": "annex_b", "model": SELECTOR, "implementation": REVISION}


class DecodingModel:
    """Scores every decoded picture with a constant rising window, so no weights are needed."""

    def score(self, video: PacketVideo, times: StageTimes) -> pa.RecordBatch:
        timestamps = np.array([time_ns for time_ns, rgb in times.timed("decode", video.pictures())], np.uint64)
        rising = np.tile(np.arange(32, dtype=np.float32) / 31, (len(timestamps) - 31, 1))
        return DenseScores(len(timestamps), 1, rising).record_batch(timestamps)


class RejectingModel:
    def score(self, video: PacketVideo, times: StageTimes) -> pa.RecordBatch:
        raise RejectedVideo("WARP requires an episode of at least 33 pictures, got 2")


def packets(episode, **changes: str | None) -> pa.Table:
    """The episode as a request table, with its contract metadata changed; None drops a key."""
    metadata = {key: value for key, value in (CONTRACT_METADATA | changes).items() if value is not None}
    return pa.table(
        {
            "payload": pa.array(episode.payloads, pa.binary()),
            "decode_index": pa.array(range(len(episode.payloads)), pa.int64()),
            "pts_ns": pa.array(episode.times_ns, pa.int64()),
        }
    ).replace_schema_metadata(metadata)


def request(table: pa.Table) -> bytes:
    sink = pa.BufferOutputStream()
    with pa.ipc.new_stream(sink, table.schema) as writer:
        writer.write_table(table)
    return sink.getvalue().to_pybytes()


@pytest.fixture
def client() -> TestClient:
    return TestClient(ProgressService(DecodingModel(), "secret").app())


def test_health_needs_no_token(client):
    health = client.get("/health").json()
    assert health["model"] == SELECTOR and health["implementation"]
    assert health["revision"] == "warp-progress-0.2.0-fp16"


def test_scores_one_row_per_picture_in_display_order(client, episode):
    response = client.post("/score_progress", content=request(packets(episode)), headers=AUTHORIZED)
    assert response.status_code == 200 and response.headers["content-type"] == ARROW_STREAM
    scores = pa.ipc.open_stream(response.content).read_all()
    assert scores.schema == OUTPUT_SCHEMA
    assert scores["frame_index"].to_pylist() == list(range(40))
    assert scores["timestamp_ns"].to_pylist() == sorted(episode.times_ns)
    stages = [entry.split(";dur=")[0] for entry in response.headers["server-timing"].split(", ")]
    assert stages == ["request", "decode", "response"]


@pytest.mark.parametrize("authorization", [None, "Bearer wrong", "secret"])
def test_rejects_a_missing_or_wrong_token(client, episode, authorization):
    headers = {"Content-Type": ARROW_STREAM} | ({"Authorization": authorization} if authorization else {})
    assert client.post("/score_progress", content=request(packets(episode)), headers=headers).status_code == 401


@pytest.mark.parametrize(
    "change", [{"codec": "h265"}, {"framing": "avcc"}, {"model": "uynitsuj/warp-rm-sim-bottles-sss15@main"}]
)
def test_rejects_another_codec_framing_or_model(client, episode, change):
    response = client.post("/score_progress", content=request(packets(episode, **change)), headers=AUTHORIZED)
    assert response.status_code == 422 and "unsupported" in response.json()["error"]


@pytest.mark.parametrize("revision", [None, "warp-progress-0.1.0"])
def test_answers_a_request_for_another_revision_with_409(client, episode, revision):
    body = request(packets(episode, implementation=revision, codec="h265"))
    response = client.post("/score_progress", content=body, headers=AUTHORIZED)
    assert response.status_code == 409
    assert response.json() == {
        "error": f"expected implementation revision 'warp-progress-0.2.0-fp16', received {revision!r}"
    }


def test_rejects_a_body_that_is_not_an_arrow_stream(client):
    response = client.post("/score_progress", content=b"not arrow", headers=AUTHORIZED)
    assert response.status_code == 422


def test_rejects_packets_out_of_decode_order(client, episode):
    table = packets(episode)
    reversed_order = pa.array(range(len(table) - 1, -1, -1), pa.int64())
    response = client.post(
        "/score_progress", content=request(table.set_column(1, "decode_index", reversed_order)), headers=AUTHORIZED
    )
    assert response.status_code == 422


def test_reports_a_rejected_video_as_422(episode):
    client = TestClient(ProgressService(RejectingModel(), "secret").app())
    response = client.post("/score_progress", content=request(packets(episode)), headers=AUTHORIZED)
    assert response.status_code == 422
    assert response.json() == {"error": "WARP requires an episode of at least 33 pictures, got 2"}
