"""The HTTP contract Studio calls: one episode's top-camera packets in, one scored row per picture out."""

import hmac
import logging

import numpy as np
import pyarrow as pa
from fastapi import FastAPI, Request, Response
from fastapi.responses import JSONResponse
from starlette.concurrency import run_in_threadpool

from warp_progress.artifacts import SELECTOR
from warp_progress.timing import StageTimes
from warp_progress.video import PacketVideo, RejectedVideo
from warp_progress.warp import WarpModel

LOG = logging.getLogger(__name__)
ARROW_STREAM = "application/vnd.apache.arrow.stream"
IMPLEMENTATION = "warp-progress 0.2.0: PyTorch port of SpiralDB spql.ai.warp_rm, fp16 on CUDA"
# Requests name the revision they were built for, so Studio never stores answers from another one.
REVISION = "warp-progress-0.2.0-fp16"
REQUEST_METADATA = {"codec": "h264", "framing": "annex_b", "model": SELECTOR}
REQUEST_COLUMNS = {"payload": pa.binary(), "decode_index": pa.int64(), "pts_ns": pa.int64()}


class RevisionMismatch(Exception):
    """A request built for another revision of this service: a run error, not a rejected video."""


class ProgressService:
    """`POST /score_progress` for callers holding the bearer token, and an open `GET /health`."""

    def __init__(self, model: WarpModel, token: str):
        self._model = model
        self._authorization = f"Bearer {token}".encode()

    def app(self) -> FastAPI:
        app = FastAPI()

        @app.get("/health")
        def health() -> dict[str, str]:
            return {"model": SELECTOR, "implementation": IMPLEMENTATION, "revision": REVISION}

        @app.post("/score_progress")
        async def score_progress(request: Request) -> Response:
            if not self._authorized(request.headers.get("authorization", "")):
                return JSONResponse({"error": "missing or invalid bearer token"}, status_code=401)
            times = StageTimes()
            try:
                with times.stage("request"):
                    video = self._video(await request.body())
                scores = await run_in_threadpool(self._model.score, video, times)
            except RevisionMismatch as mismatch:
                return JSONResponse({"error": str(mismatch)}, status_code=409)
            except RejectedVideo as rejection:
                return JSONResponse({"error": str(rejection)}, status_code=422)
            with times.stage("response"):
                body = self._arrow_stream(scores)
            LOG.info("scored %d pictures: %s", scores.num_rows, times.server_timing())
            return Response(body, media_type=ARROW_STREAM, headers={"Server-Timing": times.server_timing()})

        return app

    def _authorized(self, header: str) -> bool:
        return hmac.compare_digest(header.encode(), self._authorization)

    @staticmethod
    def _video(body: bytes) -> PacketVideo:
        """The episode an Arrow IPC request body describes, if it follows the contract."""
        try:
            table = pa.ipc.open_stream(body).read_all()
        except pa.ArrowInvalid as error:
            raise RejectedVideo(f"request body is not an Arrow IPC stream: {error}") from error
        metadata = {key.decode(): value.decode() for key, value in (table.schema.metadata or {}).items()}
        if metadata.get("implementation") != REVISION:
            raise RevisionMismatch(
                f"expected implementation revision {REVISION!r}, received {metadata.get('implementation')!r}"
            )
        for key, expected in REQUEST_METADATA.items():
            if metadata.get(key) != expected:
                raise RejectedVideo(f"unsupported {key} {metadata.get(key)!r}; expected {expected!r}")
        for name, dtype in REQUEST_COLUMNS.items():
            if name not in table.column_names or table[name].type != dtype or table[name].null_count:
                raise RejectedVideo(f"request needs a non-null {dtype} column {name}")
        if not np.array_equal(table["decode_index"].to_numpy(), np.arange(len(table))):
            raise RejectedVideo("packets must be sorted by their 0-based decode_index")
        return PacketVideo(table["payload"].to_pylist(), table["pts_ns"].to_pylist())

    @staticmethod
    def _arrow_stream(batch: pa.RecordBatch) -> bytes:
        sink = pa.BufferOutputStream()
        with pa.ipc.new_stream(sink, batch.schema) as writer:
            writer.write_batch(batch)
        return sink.getvalue().to_pybytes()
