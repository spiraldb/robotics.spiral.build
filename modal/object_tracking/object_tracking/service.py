"""The HTTP contract Studio calls: one episode's camera packets and what to track in, one row of
tracked objects per sampled picture out."""

from typing import Protocol

import pyarrow as pa
from fastapi import FastAPI
from video_service.episode import RejectedVideo
from video_service.service import EpisodeRequest, VideoContract, VideoService
from video_service.timing import StageTimes

from object_tracking.artifacts import SELECTOR
from object_tracking.video import SampledPicture, SampledVideo

IMPLEMENTATION = (
    "object-tracking 0.1.0: SAM 3.1 multiplex at its released 1008 pixels, five pictures a second, "
    "two MPS workers per H100"
)
# Requests name the revision they were built for, so Studio never stores tracks from another one.
REVISION = "object-tracking-0.1.0"
CONTRACT = VideoContract(SELECTOR, REVISION, IMPLEMENTATION)
# SAM 3.1 tracks what a short noun phrase names.
MAX_PROMPT_CHARS = 200


class Tracker(Protocol):
    def track(self, pictures: list[SampledPicture], prompt: str) -> pa.RecordBatch: ...


class TrackingService:
    """`POST /track_objects` for callers holding the bearer token, and an open `GET /health`. A
    request names what to track as schema metadata `prompt`."""

    def __init__(self, tracker: Tracker, token: str):
        self._tracker = tracker
        self._token = token

    def app(self) -> FastAPI:
        return VideoService(CONTRACT, self._token).app("/track_objects", self._track)

    def _track(self, episode: EpisodeRequest, times: StageTimes) -> pa.RecordBatch:
        prompt = (episode.metadata("prompt") or "").strip()
        if not prompt or len(prompt) > MAX_PROMPT_CHARS:
            raise RejectedVideo(f"name what to track in at most {MAX_PROMPT_CHARS} characters")
        with times.stage("decode"):
            pictures = SampledVideo(episode.video).pictures()
        with times.stage("track"):
            return self._tracker.track(pictures, prompt)
