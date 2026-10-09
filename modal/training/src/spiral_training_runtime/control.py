"""The control endpoint Studio calls: it takes stream uploads, starts, reports, stops, and deletes
runs, and publishes progress models. See docs/training.md#endpoints.
"""

from __future__ import annotations

import hmac
import json
import re
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Protocol

from fastapi import APIRouter, Depends, FastAPI, Header, Request, Response
from fastapi import Path as PathParameter
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from pydantic import ValidationError
from starlette.concurrency import run_in_threadpool
from starlette.exceptions import HTTPException

from .errors import RuntimeRequestError
from .publish import ModelPublisher, ModelPublishRequest
from .report import RunHistory, RunStatus
from .request import (
    NAME_PATTERN,
    STREAM_KEY_PATTERN,
    ModelKind,
    RunKey,
    RunRequest,
    StreamKey,
    WireModel,
)
from .stop import StopFlags
from .streams import UploadedStream
from .volume import TrainingVolume

API_VERSION = 1
# ACT's GPU, which `/v1/health` reports.
GPU = "L4"
# A progress model trains on batches of 1,024 32-step windows in fp32, as upstream does, which need
# more than an L4's 24 GB.
PROGRESS_GPU = "H100"
DEAD_CALL_MESSAGE = "The training job ended without reporting why; its Modal logs say."
STOPPING_MESSAGE = "Stopping: the job saves a checkpoint after its current step."

ProjectId = Annotated[str, PathParameter(pattern=NAME_PATTERN)]
RunId = Annotated[str, PathParameter(pattern=NAME_PATTERN)]
StreamKeyParameter = Annotated[str, PathParameter(pattern=STREAM_KEY_PATTERN)]


class TrainingCalls(Protocol):
    """The GPU jobs' function calls, one function per model family; Modal's in production."""

    def spawn(self, kind: ModelKind, request_json: str) -> str:
        """Starts a run's job on its family's function and answers its call ID."""
        ...

    def ended(self, call_id: str) -> bool:
        """Whether the call finished, failed, or was cancelled, with no retry left to run."""
        ...

    def cancel(self, call_id: str) -> None: ...


@dataclass(frozen=True)
class RecordedRun:
    """A run as the Volume holds it: what its job last reported, and its call."""

    status: RunStatus
    call_id: str | None


class RunControl:
    """Starts, reports, stops, and deletes the runs on the Volume."""

    def __init__(self, volume: TrainingVolume, calls: TrainingCalls, stop_flags: StopFlags) -> None:
        self._volume = volume
        self._calls = calls
        self._stop_flags = stop_flags
        # Two posts of one new run must not both spawn a job for it.
        self._creating = threading.Lock()

    def create(self, body: bytes) -> tuple[RunStatus, bool]:
        """The posted run's status, and whether this post started it."""
        request = RunRequest.model_validate_json(body).root
        key = request.key
        files = self._volume.run(key)
        with self._creating:
            with self._volume.reading():
                posted = files.read_request()
            if posted is not None:
                if json.loads(posted) != json.loads(body):
                    raise RuntimeRequestError(
                        f"run {key.run_id} already exists with a different request", 409
                    )
                return self._observe([key])[key], False
            missing = self._volume.missing_streams(request.stream_keys())
            if missing:
                raise RuntimeRequestError(
                    f"the run reads {len(missing)} streams the service lacks: {', '.join(missing)}",
                    422,
                )
            call_id = self._calls.spawn(request.config.model.kind, body.decode())
            with self._volume.writing():
                files.write_request(body)
                files.write_call_id(call_id)
        return RunStatus.starting(key.run_id), True

    def statuses(self, keys: list[RunKey]) -> list[RunStatus]:
        return list(self._observe(keys).values())

    def history(self, key: RunKey) -> RunHistory | None:
        status = self._observe([key]).get(key)
        if status is None:
            return None
        with self._volume.reading():
            history = self._volume.run(key).read_history() or {"metrics": [], "logs": []}
        return RunHistory(**status.model_dump(), **history)

    def stop(self, key: RunKey) -> RunStatus | None:
        status = self._observe([key]).get(key)
        if status is None or not status.active or status.phase == "stopping":
            return status
        if status.phase != "starting":
            self._stop_flags[key.flag] = True
            return status.reported_as("stopping", STOPPING_MESSAGE)
        # The job has not started, so there is nothing to checkpoint: cancel it instead.
        files = self._volume.run(key)
        with self._volume.reading():
            call_id = files.read_call_id()
        self._calls.cancel(call_id)
        stopped = status.reported_as("stopped", "Stopped before its job started.")
        with self._volume.writing():
            files.write_status(stopped)
        return stopped

    def delete(self, key: RunKey) -> None:
        status = self._observe([key]).get(key)
        if status is None:
            return
        if status.active:
            raise RuntimeRequestError(f"run {key.run_id} is {status.phase}; stop it first", 409)
        with self._volume.writing():
            self._volume.run(key).delete()
        self._stop_flags.pop(key.flag, None)

    def _observe(self, keys: list[RunKey]) -> dict[RunKey, RunStatus]:
        """Each held run's status as Studio should see it."""
        runs = self._recorded(keys)
        ended = {
            key
            for key, run in runs.items()
            if run.status.active and (run.call_id is None or self._calls.ended(run.call_id))
        }
        if ended:
            # A job commits its last status before its call ends, so reading again now sees it.
            runs.update(self._recorded(list(ended)))
        return {key: self._reported(key, run.status, key in ended) for key, run in runs.items()}

    def _recorded(self, keys: list[RunKey]) -> dict[RunKey, RecordedRun]:
        recorded = {}
        with self._volume.reading():
            for key in keys:
                files = self._volume.run(key)
                if files.exists():
                    status = files.read_status() or RunStatus.starting(key.run_id)
                    recorded[key] = RecordedRun(status, files.read_call_id())
        return recorded

    def _reported(self, key: RunKey, status: RunStatus, call_ended: bool) -> RunStatus:
        if not status.active:
            return status
        if call_ended:
            return status.reported_as("failed", DEAD_CALL_MESSAGE)
        if status.phase in {"preparing", "running"} and self._stop_flags.get(key.flag, False):
            return status.reported_as("stopping", STOPPING_MESSAGE)
        return status


class StreamKeys(WireModel):
    keys: list[StreamKey]


class ControlService:
    """The HTTP contract: a bearer token on everything but health, and `{"error": …}` answers."""

    def __init__(
        self, volume: TrainingVolume, runs: RunControl, publisher: ModelPublisher, token: str
    ) -> None:
        self._volume = volume
        self._runs = runs
        self._publisher = publisher
        self._authorization = f"Bearer {token}".encode()

    def app(self) -> FastAPI:
        app = FastAPI(title="Spiral Robotics training")
        app.add_exception_handler(RuntimeRequestError, self._runtime_error)
        app.add_exception_handler(RequestValidationError, self._invalid_request)
        app.add_exception_handler(ValidationError, self._invalid_request)
        app.add_exception_handler(HTTPException, self._http_error)

        @app.get("/v1/health")
        def health() -> dict[str, int | str]:
            return {"api_version": API_VERSION, "gpu": GPU}

        app.include_router(self._authorized_routes())
        return app

    def _authorized_routes(self) -> APIRouter:
        routes = APIRouter(dependencies=[Depends(self._authorize)])

        @routes.post("/v1/streams/missing")
        def missing_streams(body: StreamKeys) -> dict[str, list[str]]:
            return {"missing": self._volume.missing_streams(body.keys)}

        @routes.put("/v1/streams/{key}", status_code=204)
        async def put_stream(key: StreamKeyParameter, request: Request) -> Response:
            # Received to local disk first: see `TrainingVolume.store_stream`.
            with tempfile.NamedTemporaryFile(suffix=".arrow", delete_on_close=False) as upload:
                async for chunk in request.stream():
                    upload.write(chunk)
                upload.close()
                await run_in_threadpool(self._store_stream, key, Path(upload.name))
            return Response(status_code=204)

        @routes.post("/v1/runs")
        async def create_run(request: Request) -> JSONResponse:
            body = await request.body()
            status, created = await run_in_threadpool(self._runs.create, body)
            return JSONResponse(status.model_dump(), status_code=201 if created else 200)

        @routes.get("/v1/runs/{project}")
        def list_runs(project: ProjectId, ids: str) -> list[RunStatus]:
            run_ids = [run_id for run_id in ids.split(",") if run_id]
            invalid = [run_id for run_id in run_ids if not re.fullmatch(NAME_PATTERN, run_id)]
            if invalid:
                raise RuntimeRequestError(f"invalid run IDs: {', '.join(invalid)}", 422)
            return self._runs.statuses([RunKey(project, run_id) for run_id in run_ids])

        @routes.get("/v1/runs/{project}/{run_id}")
        def run_history(project: ProjectId, run_id: RunId) -> RunHistory:
            return self._held(self._runs.history(RunKey(project, run_id)), run_id)

        @routes.post("/v1/runs/{project}/{run_id}/stop")
        def stop_run(project: ProjectId, run_id: RunId) -> RunStatus:
            return self._held(self._runs.stop(RunKey(project, run_id)), run_id)

        @routes.delete("/v1/runs/{project}/{run_id}", status_code=204)
        def delete_run(project: ProjectId, run_id: RunId) -> Response:
            self._runs.delete(RunKey(project, run_id))
            return Response(status_code=204)

        @routes.post("/v1/models")
        def publish_model(body: ModelPublishRequest) -> JSONResponse:
            published, created = self._publisher.publish(body)
            return JSONResponse(published.model_dump(), status_code=201 if created else 200)

        return routes

    def _authorize(self, authorization: Annotated[str, Header()] = "") -> None:
        if not hmac.compare_digest(authorization.encode(), self._authorization):
            raise RuntimeRequestError("missing or invalid bearer token", 401)

    def _store_stream(self, key: str, upload: Path) -> None:
        UploadedStream.check(upload)
        self._volume.store_stream(key, upload)

    @staticmethod
    def _held[Answer](answer: Answer | None, run_id: str) -> Answer:
        if answer is None:
            raise RuntimeRequestError(f"run {run_id} is unknown", 404)
        return answer

    @staticmethod
    async def _runtime_error(_request: Request, error: RuntimeRequestError) -> JSONResponse:
        return JSONResponse({"error": str(error)}, status_code=error.status_code)

    @staticmethod
    async def _invalid_request(
        _request: Request, error: RequestValidationError | ValidationError
    ) -> JSONResponse:
        problems = "; ".join(
            f"{'.'.join(map(str, problem['loc']))}: {problem['msg']}"
            if problem["loc"]
            else problem["msg"]
            for problem in error.errors()
        )
        return JSONResponse({"error": problems}, status_code=422)

    @staticmethod
    async def _http_error(_request: Request, error: HTTPException) -> JSONResponse:
        return JSONResponse({"error": error.detail}, status_code=error.status_code)
