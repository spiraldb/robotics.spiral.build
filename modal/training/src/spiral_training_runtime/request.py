"""The run request Studio posts: which model family a run trains, on what, and how it sees it.

`config.model.kind` names the family: an ACT policy (`run-request.json`) or a progress model
(`progress-run-request.json`); both fixtures are in `crates/backend/src/training/fixtures`. See
`docs/training.md`.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Annotated, Any, Literal, Self

from pydantic import (
    BaseModel,
    ConfigDict,
    Discriminator,
    Field,
    RootModel,
    StringConstraints,
    Tag,
    model_validator,
)

# A stream key is the lowercase hex SHA-256 of what was read.
STREAM_KEY_PATTERN = r"^[0-9a-f]{64}$"
# Project and run IDs name Volume directories, so each stays one path component.
NAME_PATTERN = r"^[A-Za-z0-9][A-Za-z0-9._-]*$"

StreamKey = Annotated[str, StringConstraints(pattern=STREAM_KEY_PATTERN)]
VolumeName = Annotated[str, StringConstraints(pattern=NAME_PATTERN)]
ModelKind = Literal["act", "progress"]


class WireModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ImageSize(WireModel):
    width: int = Field(gt=0)
    height: int = Field(gt=0)


class Observation(WireModel):
    """How an ACT policy sees the data: which cameras, at what size, on what clock."""

    cameras: list[str] = Field(min_length=1)
    image: ImageSize
    rate_hz: float = Field(gt=0, allow_inf_nan=False)


class Optimization(WireModel):
    steps: int = Field(gt=0)
    batch_size: int = Field(gt=0)
    validation_split: float = Field(ge=0, lt=1)
    learning_rate: float = Field(gt=0, allow_inf_nan=False)
    checkpoint_every: int = Field(gt=0)
    seed: int = Field(ge=0)


class ActArchitecture(WireModel):
    hidden_dim: int = Field(gt=0)
    latent_dim: int = Field(gt=0)
    encoder_layers: int = Field(gt=0)
    vae_encoder_layers: int = Field(gt=0)
    decoder_layers: int = Field(gt=0)
    attention_heads: int = Field(gt=0)
    feedforward_dim: int = Field(gt=0)
    kl_weight: float = Field(ge=0, allow_inf_nan=False)
    pretrained_backbone: bool


class ActModel(WireModel):
    """An ACT policy: how it sees the data, how far ahead it acts, and its shape."""

    kind: Literal["act"]
    observation: Observation
    chunk_size: int = Field(gt=0)
    architecture: ActArchitecture


class ProgressModel(WireModel):
    """WARP-RM's progress head, trained on one camera's video; the recipe fixes the rest."""

    kind: Literal["progress"]
    camera: str = Field(min_length=1)


class TrainingConfig(WireModel):
    """Studio's `TrainingConfig`, exactly as it serializes it; each family's adds its `model`."""

    run_name: str
    # Which view of which dataset is Studio's business; the episodes carry what it resolved to.
    source: dict[str, Any]
    optimization: Optimization


class ActConfig(TrainingConfig):
    model: ActModel


class ProgressConfig(TrainingConfig):
    model: ProgressModel


class Joints(WireModel):
    """What each state and action value is, in order."""

    state: list[str] = Field(min_length=1)
    action: list[str] = Field(min_length=1)


class Episode(WireModel):
    """One episode and the intervals its view keeps of it."""

    episode_id: str
    intervals: list[tuple[int, int]] = Field(min_length=1)

    @model_validator(mode="after")
    def _intervals_are_not_empty(self) -> Self:
        if any(start >= end for start, end in self.intervals):
            raise ValueError(f"episode {self.episode_id} has an empty interval")
        return self


class ActEpisode(Episode):
    """An ACT episode's streams: its samples come from them."""

    # One per configured camera, in the configured order.
    cameras: list[StreamKey]
    # Value streams whose columns concatenate, in order, into the state and the action.
    state: list[StreamKey] = Field(min_length=1)
    action: list[StreamKey] = Field(min_length=1)


class ProgressEpisode(Episode):
    """A progress episode's one stream: the configured camera's."""

    camera: StreamKey


@dataclass(frozen=True)
class RunKey:
    """Which run: a run ID within a project."""

    project: str
    run_id: str

    @property
    def flag(self) -> str:
        """The run's name in the stop-flag Dict."""
        return f"{self.project}/{self.run_id}"


class FamilyRunRequest(WireModel, ABC):
    """What a run request of any family carries: which run it is, how it optimizes, and the streams
    it reads."""

    project: VolumeName
    run_id: VolumeName
    # Each family's request narrows it to its own config.
    config: TrainingConfig

    @property
    def key(self) -> RunKey:
        return RunKey(self.project, self.run_id)

    @abstractmethod
    def stream_keys(self) -> list[str]:
        """Every stream the run reads, each once."""


class ActRunRequest(FamilyRunRequest):
    robot_type: str
    joints: Joints
    config: ActConfig
    episodes: list[ActEpisode] = Field(min_length=1)

    @model_validator(mode="after")
    def _streams_follow_the_config(self) -> Self:
        cameras = len(self.config.model.observation.cameras)
        features = self.camera_features()
        if len(set(features)) != len(features):
            raise ValueError(f"camera topics must name distinct features, got {features}")
        for episode in self.episodes:
            if len(episode.cameras) != cameras:
                raise ValueError(
                    f"episode {episode.episode_id} lists {len(episode.cameras)} camera streams "
                    f"for {cameras} cameras"
                )
        return self

    def camera_features(self) -> list[str]:
        """The policy's image feature for each camera topic, in the configured order.

        `/top-camera` reads as `observation.images.top_camera`.
        """
        return [
            "observation.images." + topic.lstrip("/").replace("/", "_").replace("-", "_")
            for topic in self.config.model.observation.cameras
        ]

    def stream_keys(self) -> list[str]:
        keys = (
            key
            for episode in self.episodes
            for key in (*episode.cameras, *episode.state, *episode.action)
        )
        return list(dict.fromkeys(keys))


class ProgressRunRequest(FamilyRunRequest):
    config: ProgressConfig
    episodes: list[ProgressEpisode] = Field(min_length=1)

    def stream_keys(self) -> list[str]:
        return list(dict.fromkeys(episode.camera for episode in self.episodes))


def _model_kind(request: Any) -> str | None:
    """The family a request trains, `config.model.kind`, as posted or as parsed; pydantic's
    discriminator for `RunRequest`."""
    if isinstance(request, FamilyRunRequest):
        return request.config.model.kind
    config = request.get("config") if isinstance(request, dict) else None
    model = config.get("model") if isinstance(config, dict) else None
    return model.get("kind") if isinstance(model, dict) else None


class RunRequest(
    RootModel[
        Annotated[
            Annotated[ActRunRequest, Tag("act")] | Annotated[ProgressRunRequest, Tag("progress")],
            Discriminator(
                _model_kind,
                custom_error_type="model_kind",
                custom_error_message="config.model.kind must be act or progress",
            ),
        ]
    ]
):
    """A run request of either family, told apart by `config.model.kind`; `root` is the request."""
