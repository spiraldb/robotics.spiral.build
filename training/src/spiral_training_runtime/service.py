"""The local policy runtime the robot connector calls during rollout: it loads a training
checkpoint and turns observations into action chunks."""

from __future__ import annotations

import os
import threading
import uuid
from typing import Annotated, Any

from fastapi import Body, FastAPI, Request, Response
from fastapi.responses import JSONResponse

from .act import PolicySession
from .arrow import decode_inference_input
from .errors import RuntimeRequestError

ARROW_STREAM_CONTENT_TYPE = "application/vnd.apache.arrow.stream"
POLICY_ARTIFACT_CONTENT_TYPE = "application/vnd.spiral.policy-artifact"
MAX_REQUEST_BYTES = 520 * 1024 * 1024


class PolicySessions:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._sessions: dict[str, PolicySession] = {}

    def add(self, policy: PolicySession) -> str:
        policy_id = f"policy-{uuid.uuid4().hex}"
        with self._lock:
            self._sessions[policy_id] = policy
        return policy_id

    def get(self, policy_id: str) -> PolicySession:
        with self._lock:
            policy = self._sessions.get(policy_id)
        if policy is None:
            raise RuntimeRequestError(f"policy session {policy_id!r} does not exist", 404)
        return policy

    def remove(self, policy_id: str) -> None:
        with self._lock:
            self._sessions.pop(policy_id, None)


def create_app(policy_device: str | None = None) -> FastAPI:
    app = FastAPI(title="Spiral Robotics policy runtime", docs_url="/docs", redoc_url=None)
    sessions = PolicySessions()
    policy_device = policy_device or os.environ.get("SPIRAL_POLICY_DEVICE", "auto")

    @app.exception_handler(RuntimeRequestError)
    async def runtime_error(_request: Request, error: RuntimeRequestError) -> JSONResponse:
        return JSONResponse(status_code=error.status_code, content={"error": str(error)})

    @app.get("/v1/health")
    def health() -> dict[str, str]:
        return {"service": "spiral-training-runtime", "serves": "ACT policies for rollout"}

    @app.post("/v1/policies", status_code=201)
    def create_policy(
        request: Request,
        body: Annotated[bytes, Body(media_type=POLICY_ARTIFACT_CONTENT_TYPE)],
    ) -> dict[str, Any]:
        _require_content_type(request, POLICY_ARTIFACT_CONTENT_TYPE, "policy artifacts")
        _require_body_size(body)
        config, weights = _decode_policy_artifact(body)
        policy = PolicySession.from_artifact(config, weights, policy_device)
        policy_id = sessions.add(policy)
        return {"session_id": policy_id, "specification": policy.specification}

    @app.delete("/v1/policies/{policy_id}", status_code=204)
    def delete_policy(policy_id: str) -> Response:
        sessions.remove(policy_id)
        return Response(status_code=204)

    @app.post("/v1/policies/{policy_id}/warm-up")
    def warm_up_policy(policy_id: str) -> dict[str, float]:
        return sessions.get(policy_id).warm_up()

    @app.post("/v1/policies/{policy_id}/infer")
    def infer_policy(
        policy_id: str,
        request: Request,
        body: Annotated[bytes, Body(media_type=ARROW_STREAM_CONTENT_TYPE)],
    ) -> dict[str, Any]:
        _require_content_type(request, ARROW_STREAM_CONTENT_TYPE, "Arrow tensor requests")
        _require_body_size(body)
        policy = sessions.get(policy_id)
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
