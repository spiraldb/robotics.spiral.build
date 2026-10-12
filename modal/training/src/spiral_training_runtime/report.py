"""What a run reports: where it is, its checkpoints, and its metrics and log tail.

The job writes them to the Volume and the control serves them. Their shape is fixed by
`run-status.json` and `run-history.json` in `crates/backend/src/training/fixtures`.
"""

from __future__ import annotations

from collections import deque
from dataclasses import asdict, dataclass, field
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict

Phase = Literal["starting", "preparing", "running", "stopping", "completed", "stopped", "failed"]
ACTIVE_PHASES = frozenset({"starting", "preparing", "running", "stopping"})
MAX_STEP_METRICS = 3_584
MAX_EVENT_METRICS = 512
MAX_LOG_LINES = 512


class CheckpointRecord(BaseModel):
    model_config = ConfigDict(extra="forbid")

    step: int
    validation_loss: float | None
    created_ms: int


class RunStatus(BaseModel):
    """Where a run is, and the checkpoints it has saved."""

    model_config = ConfigDict(extra="forbid")

    run_id: str
    phase: Phase
    current_step: int = 0
    message: str | None = None
    checkpoints: list[CheckpointRecord] = []

    @classmethod
    def starting(cls, run_id: str) -> RunStatus:
        """A spawned run whose job has not reported yet."""
        return cls(run_id=run_id, phase="starting", message="Waiting for a GPU.")

    @property
    def active(self) -> bool:
        return self.phase in ACTIVE_PHASES

    def reported_as(self, phase: Phase, message: str | None) -> RunStatus:
        return self.model_copy(update={"phase": phase, "message": message})


class RunHistory(RunStatus):
    """A run's status with the metrics and log tail its job retains."""

    metrics: list[dict[str, Any]]
    logs: list[str]


@dataclass
class MetricSeries:
    """A whole run's metrics in bounded memory.

    When the series fills, every other point is dropped and later points are kept half as
    often, so the retained points always span the whole run.
    """

    capacity: int
    points: list[dict[str, Any]] = field(default_factory=list)
    stride: int = 1
    offered: int = 0

    def append(self, point: dict[str, Any]) -> None:
        if self.offered % self.stride == 0:
            self.points.append(point)
        self.offered += 1
        if len(self.points) > self.capacity:
            self.points = self.points[::2]
            self.stride *= 2


@dataclass
class TrainingProgress:
    elapsed_seconds: float = 0.0
    validation_loss: float | None = None
    # The lowest validation loss yet, and the step it was measured at.
    best_validation_loss: float | None = None
    best_validation_step: int | None = None
    # The step of the checkpoint kept only for holding the lowest validation loss yet.
    best_only_checkpoint: int | None = None


class RunReport:
    """A job's account of its run as it trains: the status and the history it writes."""

    def __init__(self, run_id: str) -> None:
        self._status = RunStatus(run_id=run_id, phase="preparing")
        self._steps = MetricSeries(MAX_STEP_METRICS)
        # Validation points are rarer and each one matters, so they are kept apart.
        self._events = MetricSeries(MAX_EVENT_METRICS)
        self._logs: deque[str] = deque(maxlen=MAX_LOG_LINES)
        self._progress = TrainingProgress()

    @property
    def status(self) -> RunStatus:
        return self._status

    @property
    def elapsed_seconds(self) -> float:
        """How long the run has trained, across retried jobs."""
        return self._progress.elapsed_seconds

    def enter(self, phase: Phase, message: str) -> None:
        self._status = self._status.reported_as(phase, message)

    def record(self, metric: dict[str, Any]) -> None:
        """One optimizer step's metrics."""
        progress = self._progress
        self._status.current_step = metric["step"]
        progress.elapsed_seconds = metric["elapsed_seconds"]
        loss = metric["validation_loss"]
        if loss is not None:
            progress.validation_loss = loss
            if progress.best_validation_loss is None or loss < progress.best_validation_loss:
                progress.best_validation_loss = loss
                progress.best_validation_step = metric["step"]
        series = self._steps if metric["event"] is None else self._events
        series.append(metric)

    @property
    def at_best(self) -> bool:
        """Whether the current step measured the lowest validation loss yet, so its weights are
        the run's best."""
        return self._progress.best_validation_step == self._status.current_step

    @property
    def best_checkpoint_step(self) -> int | None:
        """The checkpoint holding the lowest validation loss yet, when one does."""
        step = self._progress.best_validation_step
        listed = any(checkpoint.step == step for checkpoint in self._status.checkpoints)
        return step if listed else None

    def add_checkpoint(self, created_ms: int, regular: bool) -> None:
        """A checkpoint of the current step, with the latest validation loss: `regular` when the
        run's interval or its end asks for it, else kept only for holding the lowest validation
        loss yet. A best checkpoint replaces the one kept only for being best before it."""
        progress = self._progress
        step = self._status.current_step
        if self.at_best:
            superseded = progress.best_only_checkpoint
            self._status.checkpoints = [
                checkpoint
                for checkpoint in self._status.checkpoints
                if checkpoint.step != superseded
            ]
            progress.best_only_checkpoint = None if regular else step
        self._status.checkpoints.append(
            CheckpointRecord(
                step=step, validation_loss=progress.validation_loss, created_ms=created_ms
            )
        )
        if regular:
            self.log(f"Checkpoint {step} saved.")
        else:
            self.log(f"Checkpoint {step} saved: the lowest validation loss yet.")

    @property
    def last_checkpoint_step(self) -> int:
        checkpoints = self._status.checkpoints
        return checkpoints[-1].step if checkpoints else 0

    def log(self, line: str) -> None:
        self._logs.append(line)

    def history(self) -> dict[str, Any]:
        """The metrics and log tail, as `history.json` holds them."""
        metrics = sorted(self._steps.points + self._events.points, key=lambda point: point["step"])
        return {"metrics": metrics, "logs": list(self._logs)}

    def state(self) -> dict[str, Any]:
        """Everything a retried job needs to report on from where this one checkpointed."""
        return {
            "status": self._status.model_dump(),
            "steps": asdict(self._steps),
            "events": asdict(self._events),
            "logs": list(self._logs),
            "progress": asdict(self._progress),
        }

    @classmethod
    def restore(cls, state: dict[str, Any]) -> RunReport:
        report = cls(state["status"]["run_id"])
        report._status = RunStatus.model_validate(state["status"])
        report._steps = MetricSeries(**state["steps"])
        report._events = MetricSeries(**state["events"])
        report._logs.extend(state["logs"])
        report._progress = TrainingProgress(**state["progress"])
        return report
