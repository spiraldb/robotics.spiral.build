"""The HTTP contract Studio's backend speaks to every video service: one episode's camera packets in
as an Arrow IPC stream, one Arrow record batch of answers out."""

import hmac
import logging
from collections.abc import Callable

import numpy as np
import pyarrow as pa
from fastapi import FastAPI, Request, Response
from fastapi.responses import JSONResponse
from starlette.concurrency import run_in_threadpool

from video_service.episode import EpisodeVideo, RejectedVideo
from video_service.timing import StageTimes

LOG = logging.getLogger(__name__)
ARROW_STREAM = "application/vnd.apache.arrow.stream"
REQUEST_COLUMNS = {"payload": pa.binary(), "decode_index": pa.int64(), "pts_ns": pa.int64()}


class RevisionMismatch(Exception):
    """A request built for another revision of the service: a run error, not a rejected video."""


class RetryLater(Exception):
    """A failure of the service rather than of the video, such as running out of GPU memory; the
    caller asks again."""


class UnknownModel(Exception):
    """A request for a model the service does not hold: a run error, not a rejected video."""


class IncompatibleModel(Exception):
    """A request for a model the service holds but cannot run as it runs: a run error, not a
    rejected video."""


class EpisodeRequest:
    """One request's episode and the schema metadata it came with."""

    def __init__(self, video: EpisodeVideo, metadata: dict[str, str]):
        self._video = video
        self._metadata = metadata

    @property
    def video(self) -> EpisodeVideo:
        return self._video

    def metadata(self, key: str) -> str | None:
        return self._metadata.get(key)


class VideoContract:
    """What one service answers for: H.264 Annex-B packets, read for its pinned `model`, by requests
    built for its `revision`. Studio stores answers under the revision, so any other is refused.

    A service that reads with more than one model pins none: it reads each request's `model` itself.
    """

    def __init__(self, model: str | None, revision: str, implementation: str):
        self._model = model
        self._metadata = {"codec": "h264", "framing": "annex_b"} | ({} if model is None else {"model": model})
        self._revision = revision
        self._implementation = implementation

    def health(self) -> dict[str, str | None]:
        return {"model": self._model, "implementation": self._implementation, "revision": self._revision}

    def episode(self, body: bytes) -> EpisodeRequest:
        """The episode an Arrow IPC request body describes, if it follows the contract."""
        try:
            table = pa.ipc.open_stream(body).read_all()
        except pa.ArrowInvalid as error:
            raise RejectedVideo(f"request body is not an Arrow IPC stream: {error}") from error
        metadata = {key.decode(): value.decode() for key, value in (table.schema.metadata or {}).items()}
        if metadata.get("implementation") != self._revision:
            raise RevisionMismatch(
                f"expected implementation revision {self._revision!r}, received {metadata.get('implementation')!r}"
            )
        for key, expected in self._metadata.items():
            if metadata.get(key) != expected:
                raise RejectedVideo(f"unsupported {key} {metadata.get(key)!r}; expected {expected!r}")
        for name, dtype in REQUEST_COLUMNS.items():
            if name not in table.column_names or table[name].type != dtype or table[name].null_count:
                raise RejectedVideo(f"request needs a non-null {dtype} column {name}")
        if not np.array_equal(table["decode_index"].to_numpy(), np.arange(len(table))):
            raise RejectedVideo("packets must be sorted by their 0-based decode_index")
        return EpisodeRequest(EpisodeVideo(table["payload"].to_pylist(), table["pts_ns"].to_pylist()), metadata)


class VideoService:
    """`POST` one route for callers holding the bearer token, and an open `GET /health`.

    The route reads its episode under the `request` stage and hands it to `answer` off the event
    loop, with the request's stage timer; the answer goes back as an Arrow IPC stream, its stages in
    a `Server-Timing` header. A rejected video is a 422, a request for another revision or an
    `IncompatibleModel` a 409, an `UnknownModel` a 404, and a `RetryLater` a 503.
    """

    def __init__(self, contract: VideoContract, token: str):
        self._contract = contract
        self._authorization = f"Bearer {token}".encode()

    def app(self, route: str, answer: Callable[[EpisodeRequest, StageTimes], pa.RecordBatch]) -> FastAPI:
        app = FastAPI()

        @app.get("/health")
        def health() -> dict[str, str | None]:
            return self._contract.health()

        @app.post(route)
        async def answer_episode(request: Request) -> Response:
            if not hmac.compare_digest(request.headers.get("authorization", "").encode(), self._authorization):
                return JSONResponse({"error": "missing or invalid bearer token"}, status_code=401)
            times = StageTimes()
            try:
                with times.stage("request"):
                    episode = self._contract.episode(await request.body())
                answers = await run_in_threadpool(answer, episode, times)
            except UnknownModel as unknown:
                return JSONResponse({"error": str(unknown)}, status_code=404)
            except (RevisionMismatch, IncompatibleModel) as mismatch:
                return JSONResponse({"error": str(mismatch)}, status_code=409)
            except RejectedVideo as rejection:
                return JSONResponse({"error": str(rejection)}, status_code=422)
            except RetryLater as failure:
                return JSONResponse({"error": str(failure)}, status_code=503)
            with times.stage("response"):
                body = self._arrow_stream(answers)
            LOG.info("%s answered %d rows: %s", route, answers.num_rows, times.server_timing())
            return Response(body, media_type=ARROW_STREAM, headers={"Server-Timing": times.server_timing()})

        return app

    @staticmethod
    def _arrow_stream(batch: pa.RecordBatch) -> bytes:
        sink = pa.BufferOutputStream()
        with pa.ipc.new_stream(sink, batch.schema) as writer:
            writer.write_batch(batch)
        return sink.getvalue().to_pybytes()
