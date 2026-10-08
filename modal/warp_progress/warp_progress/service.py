"""The HTTP contract Studio calls: one episode's top-camera packets in, one scored row per picture out."""

import pyarrow as pa
from fastapi import FastAPI
from video_service.service import EpisodeRequest, VideoContract, VideoService
from video_service.timing import StageTimes

from warp_progress.warp import WarpModel

IMPLEMENTATION = "warp-progress 0.2.0: PyTorch port of SpiralDB spql.ai.warp_rm, fp16 on CUDA"
# Requests name the revision they were built for, so Studio never stores answers from another one.
REVISION = "warp-progress-0.2.0-fp16"
# Requests name the head they are scored with: see `ProgressService`.
CONTRACT = VideoContract(None, REVISION, IMPLEMENTATION)


class ProgressService:
    """`POST /score_progress` for callers holding the bearer token, and an open `GET /health`.

    A request names the head it is scored with as schema metadata `model`: the released
    `artifacts.SELECTOR`, or a published progress model, `spiral-robotics-models/<sha256>`. One that
    names no such model is a 404, and a published model made for another scorer a 409.
    """

    def __init__(self, model: WarpModel, token: str):
        self._model = model
        self._token = token

    def app(self) -> FastAPI:
        return VideoService(CONTRACT, self._token).app("/score_progress", self._score)

    def _score(self, episode: EpisodeRequest, times: StageTimes) -> pa.RecordBatch:
        return self._model.score(episode.video, episode.metadata("model"), times)
