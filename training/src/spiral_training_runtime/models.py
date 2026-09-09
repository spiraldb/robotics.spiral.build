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
    client_id: str | None = None
    client_secret: str | None = None


class ActTrainingJobConfigModel(RuntimeModel):
    camera_streams: list[str]
    image_width: int = Field(gt=0)
    image_height: int = Field(gt=0)
    video_decoder: str = Field(min_length=1)
    hidden_dimensions: int = Field(gt=0)
    latent_dimensions: int = Field(gt=0)
    attention_heads: int = Field(gt=0)
    feedforward_dimensions: int = Field(gt=0)
    encoder_layers: int = Field(gt=0)
    vae_encoder_layers: int = Field(gt=0)
    decoder_layers: int = Field(gt=0)
    kl_weight: float = Field(ge=0, allow_inf_nan=False)


class TrainingJobRequestModel(RuntimeModel):
    job_id: str = Field(min_length=1)
    run_name: str = Field(min_length=1)
    spiral: SpiralJobConnectionModel
    sql: str = Field(min_length=1)
    state_column: str = Field(min_length=1)
    action_column: str = Field(min_length=1)
    spiral_schema_version: int = Field(
        ge=VERSIONS.training_protocol.storage_schema.minimum,
        le=VERSIONS.training_protocol.storage_schema.maximum,
    )
    device: str = Field(min_length=1)
    initialization: Literal["random", "pretrained_backbone"]
    steps: int = Field(gt=0)
    batch_size: int = Field(gt=0)
    validation_split: float = Field(ge=0, lt=1, allow_inf_nan=False)
    learning_rate: float = Field(gt=0, allow_inf_nan=False)
    checkpoint_every: int = Field(gt=0)
    chunk_size: int = Field(gt=0)
    seed: int = Field(ge=0)
    act: ActTrainingJobConfigModel


class TrainingJobCursorModel(RuntimeModel):
    metric_step: int = Field(ge=0)
    checkpoint_step: int = Field(ge=0)
    log_offset: int = Field(ge=0)


class TrainingJobResolvedModel(RuntimeModel):
    camera_streams: list[str]
    state_dimensions: int
    action_dimensions: int
    image_width: int
    image_height: int
    sample_hz: float
    robot_type: str
    joint_names: list[str]
    parameter_count: int
    device: str


class TrainingJobMetricModel(RuntimeModel):
    step: int
    train_loss: float
    validation_loss: float | None
    learning_rate: float
    examples_per_second: float
    cpu_percent: float | None
    gpu_percent: float | None
    memory_bytes: int | None
    loader_ms: float | None
    device_copy_ms: float | None
    model_ms: float | None
    elapsed_seconds: float
    event: str | None


class TrainingJobCheckpointModel(RuntimeModel):
    step: int
    created_ms: int
    validation_loss: float | None
    byte_size: int
    recommended_action_steps: int
    sample_hz: float


class TrainingJobLogModel(RuntimeModel):
    offset: int
    line: str


class TrainingJobSnapshotModel(RuntimeModel):
    job_id: str
    phase: Literal["starting", "running", "stopping", "completed", "stopped", "failed"]
    current_step: int
    finished_ms: int | None
    message: str | None
    resolved: TrainingJobResolvedModel | None
    metrics: list[TrainingJobMetricModel]
    checkpoints: list[TrainingJobCheckpointModel]
    logs: list[TrainingJobLogModel]
    cursor: TrainingJobCursorModel
    history_gap: str | None = None
