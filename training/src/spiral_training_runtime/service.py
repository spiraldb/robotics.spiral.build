from __future__ import annotations

import os
import threading
import uuid
from typing import Annotated, Any

from fastapi import Body, FastAPI, Query, Request, Response
from fastapi.responses import JSONResponse

from .act import PolicySession, TrainingSession, backend_info
from .arrow import decode_inference_input, decode_training_batch
from .config import TrainingConfig
from .errors import RuntimeRequestError

ARROW_STREAM_CONTENT_TYPE = "application/vnd.apache.arrow.stream"
POLICY_ARTIFACT_CONTENT_TYPE = "application/vnd.spiral.policy-artifact"
MAX_REQUEST_BYTES = 520 * 1024 * 1024


class RuntimeState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.sessions: dict[str, TrainingSession] = {}
        self.policies: dict[str, PolicySession] = {}

    def add_session(self, session: TrainingSession) -> str:
        session_id = f"session-{uuid.uuid4().hex}"
        with self.lock:
            self.sessions[session_id] = session
        return session_id

    def session(self, session_id: str) -> TrainingSession:
        with self.lock:
            session = self.sessions.get(session_id)
        if session is None:
            raise RuntimeRequestError(f"training session {session_id!r} does not exist", 404)
        return session

    def remove_session(self, session_id: str) -> None:
        with self.lock:
            self.sessions.pop(session_id, None)

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
        version="1",
        docs_url="/docs",
        redoc_url=None,
    )
    state = RuntimeState()
    policy_device = policy_device or os.environ.get("SPIRAL_POLICY_DEVICE", "auto")

    @app.exception_handler(RuntimeRequestError)
    async def runtime_error(_request: Request, error: RuntimeRequestError) -> JSONResponse:
        return JSONResponse(status_code=error.status_code, content={"error": str(error)})

    @app.get("/v1/health")
    def health() -> dict[str, Any]:
        return backend_info()

    @app.post("/v1/sessions", status_code=201)
    def create_session(request: dict[str, Any]) -> dict[str, Any]:
        session = TrainingSession(TrainingConfig.from_request(request))
        session_id = state.add_session(session)
        return {"session_id": session_id, "info": session.info}

    @app.delete("/v1/sessions/{session_id}", status_code=204)
    def delete_session(session_id: str) -> Response:
        state.remove_session(session_id)
        return Response(status_code=204)

    @app.get("/v1/sessions/{session_id}/telemetry")
    def telemetry(session_id: str) -> dict[str, Any]:
        return state.session(session_id).telemetry()

    @app.post("/v1/sessions/{session_id}/train")
    def train(
        session_id: str,
        request: Request,
        body: Annotated[bytes, Body(media_type=ARROW_STREAM_CONTENT_TYPE)],
        step: Annotated[int, Query(gt=0)],
    ) -> dict[str, Any]:
        _require_content_type(request, ARROW_STREAM_CONTENT_TYPE, "Arrow tensor requests")
        _require_body_size(body)
        session = state.session(session_id)
        return session.train(decode_training_batch(body, session.config), step)

    @app.post("/v1/sessions/{session_id}/validate")
    def validate(
        session_id: str,
        request: Request,
        body: Annotated[bytes, Body(media_type=ARROW_STREAM_CONTENT_TYPE)],
    ) -> dict[str, Any]:
        _require_content_type(request, ARROW_STREAM_CONTENT_TYPE, "Arrow tensor requests")
        _require_body_size(body)
        session = state.session(session_id)
        return session.validate(decode_training_batch(body, session.config))

    @app.post("/v1/sessions/{session_id}/checkpoint")
    def checkpoint(session_id: str, request: dict[str, Any]) -> Response:
        payload = state.session(session_id).checkpoint(request)
        return Response(
            content=payload,
            media_type="application/octet-stream",
            headers={
                "content-disposition": "attachment; filename=checkpoint.safetensors",
                "x-spiral-checkpoint-format": "safetensors",
                "x-spiral-checkpoint-extension": "safetensors",
            },
        )

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
