"""The run request Studio posts: what a run trains on and how its model sees it.

Its shape is fixed by `crates/backend/src/training/fixtures/run-request.json`; see
`docs/training.md`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Annotated, Any, Literal, Self

from pydantic import BaseModel, ConfigDict, Field, StringConstraints, model_validator

# A stream key is the lowercase hex SHA-256 of what was read.
STREAM_KEY_PATTERN = r"^[0-9a-f]{64}$"
# Project and run IDs name Volume directories, so each stays one path component.
NAME_PATTERN = r"^[A-Za-z0-9][A-Za-z0-9._-]*$"

StreamKey = Annotated[str, StringConstraints(pattern=STREAM_KEY_PATTERN)]
VolumeName = Annotated[str, StringConstraints(pattern=NAME_PATTERN)]


class WireModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ImageSize(WireModel):
    width: int = Field(gt=0)
    height: int = Field(gt=0)


class Observation(WireModel):
    """How the model sees the data: which cameras, at what size, on what clock."""

    cameras: list[str] = Field(min_length=1)
    image: ImageSize
    rate_hz: float = Field(gt=0, allow_inf_nan=False)


class Optimization(WireModel):
    steps: int = Field(gt=0)
    batch_size: int = Field(gt=0)
    validation_split: float = Field(ge=0, lt=1)
    learning_rate: float = Field(gt=0, allow_inf_nan=False)
    checkpoint_every: int = Field(gt=0)
    chunk_size: int = Field(gt=0)
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


class TrainingConfig(WireModel):
    """Studio's `TrainingConfig`, exactly as it serializes it."""

    run_name: str
    policy: Literal["act"]
    # Which view of which dataset is Studio's business; the episodes carry what it resolved to.
    source: dict[str, Any]
    observation: Observation
    optimization: Optimization
    act: ActArchitecture


class Joints(WireModel):
    """What each state and action value is, in order."""

    state: list[str] = Field(min_length=1)
    action: list[str] = Field(min_length=1)


class Episode(WireModel):
    """One episode's kept intervals and the streams its samples come from."""

    episode_id: str
    intervals: list[tuple[int, int]] = Field(min_length=1)
    # One per configured camera, in the configured order.
    cameras: list[StreamKey]
    # Value streams whose columns concatenate, in order, into the state and the action.
    state: list[StreamKey] = Field(min_length=1)
    action: list[StreamKey] = Field(min_length=1)


@dataclass(frozen=True)
class RunKey:
    """Which run: a run ID within a project."""

    project: str
    run_id: str

    @property
    def flag(self) -> str:
        """The run's name in the stop-flag Dict."""
        return f"{self.project}/{self.run_id}"


class RunRequest(WireModel):
    project: VolumeName
    run_id: VolumeName
    robot_type: str
    joints: Joints
    config: TrainingConfig
    episodes: list[Episode] = Field(min_length=1)

    @model_validator(mode="after")
    def _streams_follow_the_config(self) -> Self:
        cameras = len(self.config.observation.cameras)
        features = self.camera_features()
        if len(set(features)) != len(features):
            raise ValueError(f"camera topics must name distinct features, got {features}")
        for episode in self.episodes:
            if len(episode.cameras) != cameras:
                raise ValueError(
                    f"episode {episode.episode_id} lists {len(episode.cameras)} camera streams "
                    f"for {cameras} cameras"
                )
            if any(start >= end for start, end in episode.intervals):
                raise ValueError(f"episode {episode.episode_id} has an empty interval")
        return self

    @property
    def key(self) -> RunKey:
        return RunKey(self.project, self.run_id)

    def camera_features(self) -> list[str]:
        """The policy's image feature for each camera topic, in the configured order.

        `/top-camera` reads as `observation.images.top_camera`.
        """
        return [
            "observation.images." + topic.lstrip("/").replace("/", "_").replace("-", "_")
            for topic in self.config.observation.cameras
        ]

    def stream_keys(self) -> list[str]:
        """Every stream the run reads, each once."""
        keys = (
            key
            for episode in self.episodes
            for key in (*episode.cameras, *episode.state, *episode.action)
        )
        return list(dict.fromkeys(keys))
