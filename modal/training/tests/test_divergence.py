"""Telling a diverged run's training loss from a noisy one."""

from __future__ import annotations

import math
import random

import pytest

from spiral_training_runtime.divergence import Divergence, DivergenceWatch


def watched(losses: list[float]) -> Divergence | None:
    """The divergence a watch reports over `losses`, steps counted from 1."""
    watch = DivergenceWatch()
    for step, loss in enumerate(losses, 1):
        divergence = watch.observe(step, loss)
        if divergence is not None:
            return divergence
    return None


def test_a_falling_noisy_loss_with_spikes_never_diverges() -> None:
    noise = random.Random(0)
    losses = [
        3.0 * math.exp(-step / 2_000) + 0.5 + noise.uniform(-0.05, 0.05) for step in range(15_000)
    ]
    # Lone spikes every few hundred steps, as healthy runs show.
    for step in range(3_000, 15_000, 400):
        losses[step] = 2.5
    assert watched(losses) is None


def test_a_burst_the_run_recovers_from_is_not_a_divergence() -> None:
    # The 100-step average rises past three times its lowest, but for under 250 steps.
    assert watched([0.52] * 1_000 + [8.0] * 20 + [0.52] * 1_000) is None


def test_a_loss_that_jumps_and_stays_up_diverges_once_it_has_held() -> None:
    divergence = watched([0.52] * 1_000 + [2.9] * 1_000)

    # The average passes 3 x 0.52 with the 44th risen step; 250 steps later the window holds
    # only the risen loss.
    assert divergence is not None
    assert divergence.step == 1_044
    assert divergence.lowest == pytest.approx(0.52)
    assert divergence.risen == pytest.approx(2.9)
