from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field

from .protocol import VERSIONS


class RuntimeModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class TrainingBackendInfoModel(RuntimeModel):
    api_id: str = Field(min_length=1)
    api_version: int = Field(gt=0)
    api_revision: int = Field(ge=0)
    spiral_schema_minimum: int = Field(gt=0)
    spiral_schema_maximum: int = Field(gt=0)
    id: str = Field(min_length=1)
    display_name: str = Field(min_length=1)
    runtime: str = Field(min_length=1)
    devices: list[str]


class SpiralJobConnectionModel(RuntimeModel):
    api_url: str = Field(min_length=1)
    spfs_url: str = Field(min_length=1)
    project_id: str = Field(min_length=1)
    table_prefix: str = Field(min_length=1)


class ActConfigModel(RuntimeModel):
    camera_streams: list[str]
    image_width: int = Field(ge=0)
    image_height: int = Field(ge=0)
    video_decoder: str = Field(min_length=1)
    pretrained_backbone: bool
    hidden_dim: int = Field(gt=0)
    latent_dim: int = Field(gt=0)
    encoder_layers: int = Field(gt=0)
    vae_encoder_layers: int = Field(gt=0)
    decoder_layers: int = Field(gt=0)
    attention_heads: int = Field(gt=0)
    feedforward_dim: int = Field(gt=0)
    kl_weight: float = Field(ge=0, allow_inf_nan=False)


class TrainingConfigModel(RuntimeModel):
    """Studio's run configuration, exactly as it serializes it."""

    run_name: str = Field(min_length=1)
    dataset_id: str = Field(min_length=1)
    policy: Literal["act"]
    sql: str = Field(min_length=1)
    state_column: str = Field(min_length=1)
    action_column: str = Field(min_length=1)
    device: str = Field(min_length=1)
    steps: int = Field(gt=0)
    batch_size: int = Field(gt=0)
    validation_split: float = Field(ge=0, lt=1, allow_inf_nan=False)
    learning_rate: float = Field(gt=0, allow_inf_nan=False)
    checkpoint_every: int = Field(gt=0)
    chunk_size: int = Field(gt=0)
    seed: int = Field(ge=0)
    act: ActConfigModel


class TrainingJobRequestModel(RuntimeModel):
    job_id: str = Field(min_length=1)
    spiral: SpiralJobConnectionModel
    spiral_schema_version: int = Field(
        ge=VERSIONS.training_protocol.storage_schema.minimum,
        le=VERSIONS.training_protocol.storage_schema.maximum,
    )
    config: TrainingConfigModel


class TrainingJobStateModel(RuntimeModel):
    job_id: str
    phase: Literal["starting", "running", "stopping", "completed", "stopped", "failed"]
    current_step: int
    message: str | None


class TrainingJobMetricModel(RuntimeModel):
    step: int
    train_loss: float
    validation_loss: float | None
    learning_rate: float
    elapsed_seconds: float
    event: str | None
    examples_per_second: float
    cpu_percent: float | None
    gpu_percent: float | None
    memory_bytes: int | None
    loader_ms: float | None
    device_copy_ms: float | None
    model_ms: float | None


class TrainingJobHistoryModel(TrainingJobStateModel):
    metrics: list[TrainingJobMetricModel]
    logs: list[str]
