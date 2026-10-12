"""Telling a run that has diverged from one that is merely noisy, from its training loss alone.

A diverged run's loss jumps and stays up: the model has collapsed to one output for every input
and never comes back. On WARP-RM's progress head over 45 episodes, both bf16 runs at the default
rate collapsed, one fp32 run and one bf16 run at a quarter of the rate did not: the loss, averaged
over 100 steps, rose about 5.9 times above its lowest such average and held there for thousands of
steps. No healthy run, progress or ACT, rose past 1.16 times. The rule sits between: three times,
for 250 steps.
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass

WINDOW_STEPS = 100
RISE = 3.0
HOLD_STEPS = 250


@dataclass(frozen=True)
class Divergence:
    """Where a run diverged: the step its averaged loss first rose past the rule, the lowest
    average before it, and the average once it had held."""

    step: int
    lowest: float
    risen: float

    def message(self, best_checkpoint: int | None) -> str:
        said = (
            f"Diverged at step {self.step}: the training loss rose from {self.lowest:.2f} to "
            f"{self.risen:.2f} and stayed there."
        )
        if best_checkpoint is None:
            return said
        return f"{said} Checkpoint {best_checkpoint} has the lowest validation loss."


class DivergenceWatch:
    """Watches one attempt's training loss, step by step."""

    def __init__(self) -> None:
        self._recent: deque[float] = deque(maxlen=WINDOW_STEPS)
        self._lowest = float("inf")
        self._risen_since: int | None = None

    def observe(self, step: int, loss: float) -> Divergence | None:
        """The divergence once `step`'s loss completes one, else None."""
        self._recent.append(loss)
        if len(self._recent) < WINDOW_STEPS:
            return None
        average = sum(self._recent) / WINDOW_STEPS
        if average <= RISE * self._lowest:
            self._lowest = min(self._lowest, average)
            self._risen_since = None
            return None
        if self._risen_since is None:
            self._risen_since = step
        if step - self._risen_since < HOLD_STEPS:
            return None
        return Divergence(self._risen_since, self._lowest, average)
