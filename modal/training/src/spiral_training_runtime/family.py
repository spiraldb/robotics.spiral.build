"""What a model family gives a run's job: it prepares samples and a session on the job's local disk,
and the job steps, validates, checkpoints, reports and resumes them. ACT (`act.py`) and the progress
model (`progress.py`) are the families; each runs in its own Modal function and environment, so the
job never imports either.
"""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

import numpy as np

from .device import StepResult
from .request import FamilyRunRequest


class Batch(Protocol):
    @property
    def size(self) -> int: ...


class Samples(Protocol):
    """Where a run's training and validation batches come from."""

    @property
    def has_validation(self) -> bool: ...

    def train_batches(self, seed: np.random.SeedSequence, size: int) -> Iterator[Batch]:
        """Training batches without end, built ahead of the training loop and drawn from `seed`;
        closing the iterator stops whatever builds them."""
        ...

    def validation_batch(self, rng: np.random.Generator, size: int) -> Batch: ...


class Session(Protocol):
    """A run's model and optimizer on its device."""

    @property
    def device_name(self) -> str: ...

    @property
    def learning_rate(self) -> float:
        """The rate the last step trained at."""
        ...

    def train_step(self, batch: Batch, step: int) -> StepResult: ...

    def validate(self, batch: Batch) -> StepResult: ...

    def telemetry(self) -> dict[str, Any]: ...

    def save_checkpoint(self, directory: Path, step: int) -> None:
        """Writes the step's `config.json` and `model.safetensors` into `directory`."""
        ...

    def state(self) -> dict[str, Any]:
        """What a retried job resumes from."""
        ...

    def restore(self, state: dict[str, Any]) -> None: ...


@dataclass(frozen=True)
class PreparedRun:
    """A family's prepared samples and session, with what the job reports about them."""

    samples: Samples
    session: Session
    # Log lines for a run that did not resume.
    logs: list[str]
    # The message the run reports while it trains.
    running: str


class Family(Protocol):
    @property
    def request(self) -> FamilyRunRequest: ...

    def preparing(self) -> str:
        """The message the run reports while it prepares."""
        ...

    def prepare(self, scratch: Path) -> PreparedRun:
        """Samples and a session, with `scratch` the job's local disk."""
        ...
