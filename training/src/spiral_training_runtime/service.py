from __future__ import annotations

import os
import threading
import uuid
from collections import OrderedDict
from typing import Annotated, Any

from fastapi import Body, FastAPI, Query, Request, Response
from fastapi.responses import JSONResponse

from .act import PolicySession, backend_info
from .arrow import decode_inference_input
from .errors import RuntimeRequestError
from .jobs import TrainingJob
from .models import TrainingBackendInfoModel, TrainingJobRequestModel, TrainingJobSnapshotModel
from .protocol import BOUNDARIES

ARROW_STREAM_CONTENT_TYPE = "application/vnd.apache.arrow.stream"
POLICY_ARTIFACT_CONTENT_TYPE = "application/vnd.spiral.policy-artifact"
MAX_REQUEST_BYTES = 520 * 1024 * 1024
MAX_ACTIVE_JOBS = 4
MAX_RETAINED_TERMINAL_JOBS = 64


class RuntimeState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.jobs: OrderedDict[str, TrainingJob] = OrderedDict()
        self.policies: dict[str, PolicySession] = {}

    def add_job(self, request: dict[str, Any]) -> TrainingJob:
        job_id = request.get("job_id")
        if not isinstance(job_id, str) or not job_id:
            raise RuntimeRequestError("training job_id must be non-empty", 422)
        with self.lock:
            self._prune_jobs_locked()
            existing = self.jobs.get(job_id)
            if existing is not None:
                if not existing.matches(request):
                    raise RuntimeRequestError(
                        f"training job {job_id!r} already has a different immutable specification",
                        409,
                    )
                return existing
            active_jobs = sum(not job.terminal() for job in self.jobs.values())
            if active_jobs >= MAX_ACTIVE_JOBS:
                raise RuntimeRequestError(
                    f"runtime already owns {MAX_ACTIVE_JOBS} active training jobs", 429
                )
            job = TrainingJob(request)
            self.jobs[job_id] = job
        job.start()
        return job

    def job(self, job_id: str) -> TrainingJob:
        with self.lock:
            self._prune_jobs_locked()
            job = self.jobs.get(job_id)
        if job is None:
            raise RuntimeRequestError(f"training job {job_id!r} does not exist", 404)
        return job

    def _prune_jobs_locked(self) -> None:
        terminal_ids = [job_id for job_id, job in self.jobs.items() if job.terminal()]
        for job_id in terminal_ids[:-MAX_RETAINED_TERMINAL_JOBS]:
            self.jobs.pop(job_id, None)

    def add_policy(self, policy: PolicySession) -> str:
        policy_id = f"policy-{uuid.uuid4().hex}"
        with self.lock:
            self.policies[policy_id] = policy
        return policy_id

    def policy(self, policy_id: str) -> PolicySession:
        with self.lock:
            policy = self.policies.get(policy_id)
        if policy is None:
            raise RuntimeRequestError(f"policy session {policy_id!r} does not exist", 404)
        return policy

    def remove_policy(self, policy_id: str) -> None:
        with self.lock:
            self.policies.pop(policy_id, None)


def create_app(policy_device: str | None = None) -> FastAPI:
    app = FastAPI(
        title="Spiral Robotics PyTorch Runtime",
        version=BOUNDARIES.training_api.display_version,
        docs_url="/docs",
        redoc_url=None,
    )
    state = RuntimeState()
    policy_device = policy_device or os.environ.get("SPIRAL_POLICY_DEVICE", "auto")

    @app.exception_handler(RuntimeRequestError)
    async def runtime_error(_request: Request, error: RuntimeRequestError) -> JSONResponse:
        return JSONResponse(status_code=error.status_code, content={"error": str(error)})

    @app.get("/v1/health", response_model=TrainingBackendInfoModel)
    def health() -> dict[str, Any]:
        return backend_info()

    @app.post("/v1/jobs", status_code=201, response_model=TrainingJobSnapshotModel)
    def create_job(request: TrainingJobRequestModel) -> dict[str, Any]:
        return state.add_job(request.model_dump(exclude_none=True)).snapshot()

    @app.get("/v1/jobs/{job_id}", response_model=TrainingJobSnapshotModel)
    def get_job(
        job_id: str,
        after_metric_step: Annotated[int, Query(ge=0)] = 0,
        after_checkpoint_step: Annotated[int, Query(ge=0)] = 0,
        after_log_offset: Annotated[int, Query(ge=0)] = 0,
    ) -> dict[str, Any]:
        return state.job(job_id).snapshot(
            after_metric_step, after_checkpoint_step, after_log_offset
        )

    @app.post("/v1/jobs/{job_id}/stop", response_model=TrainingJobSnapshotModel)
    def stop_job(
        job_id: str,
        after_metric_step: Annotated[int, Query(ge=0)] = 0,
        after_checkpoint_step: Annotated[int, Query(ge=0)] = 0,
        after_log_offset: Annotated[int, Query(ge=0)] = 0,
    ) -> dict[str, Any]:
        job = state.job(job_id)
        job.stop()
        return job.snapshot(after_metric_step, after_checkpoint_step, after_log_offset)

    @app.get("/v1/jobs/{job_id}/telemetry")
    def job_telemetry(job_id: str) -> dict[str, Any]:
        snapshot = state.job(job_id).snapshot()
        return {
            "phase": snapshot["phase"],
            "current_step": snapshot["current_step"],
            "latest": snapshot["metrics"][-1] if snapshot["metrics"] else None,
        }

    @app.post("/v1/policies", status_code=201)
    def create_policy(
        request: Request,
        body: Annotated[bytes, Body(media_type=POLICY_ARTIFACT_CONTENT_TYPE)],
    ) -> dict[str, Any]:
        _require_content_type(request, POLICY_ARTIFACT_CONTENT_TYPE, "policy artifacts")
        _require_body_size(body)
        config, weights = _decode_policy_artifact(body)
        policy = PolicySession.from_artifact(config, weights, policy_device)
        policy_id = state.add_policy(policy)
        return {"session_id": policy_id, "specification": policy.specification}

    @app.delete("/v1/policies/{policy_id}", status_code=204)
    def delete_policy(policy_id: str) -> Response:
        state.remove_policy(policy_id)
        return Response(status_code=204)

    @app.post("/v1/policies/{policy_id}/warm-up")
    def warm_up_policy(policy_id: str) -> dict[str, float]:
        return state.policy(policy_id).warm_up()

    @app.post("/v1/policies/{policy_id}/infer")
    def infer_policy(
        policy_id: str,
        request: Request,
        body: Annotated[bytes, Body(media_type=ARROW_STREAM_CONTENT_TYPE)],
    ) -> dict[str, Any]:
        _require_content_type(request, ARROW_STREAM_CONTENT_TYPE, "Arrow tensor requests")
        _require_body_size(body)
        policy = state.policy(policy_id)
        return policy.infer(decode_inference_input(body, policy.config))

    return app


def _require_content_type(request: Request, expected: str, label: str) -> None:
    content_type = request.headers.get("content-type", "")
    if not content_type.startswith(expected):
        raise RuntimeRequestError(f"{label} require Content-Type {expected}", 415)


def _require_body_size(body: bytes) -> None:
    if len(body) > MAX_REQUEST_BYTES:
        raise RuntimeRequestError(
            f"request body exceeds the {MAX_REQUEST_BYTES}-byte runtime limit", 413
        )


def _decode_policy_artifact(body: bytes) -> tuple[bytes, bytes]:
    if len(body) < 8:
        raise RuntimeRequestError("policy artifact is missing its config length")
    config_length = int.from_bytes(body[:8], byteorder="little", signed=False)
    config_end = 8 + config_length
    if config_end > len(body):
        raise RuntimeRequestError("policy artifact ends before its config is complete")
    config = body[8:config_end]
    weights = body[config_end:]
    if not weights:
        raise RuntimeRequestError("policy artifact contains no SafeTensors weights")
    return config, weights


app = create_app()
