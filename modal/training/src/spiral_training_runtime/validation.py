"""Validation: whole episodes held out with the run's seed, for every model family. See
docs/training.md#how-the-model-sees-the-data.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True)
class EpisodeSplit:
    """Which items train and which validate, numbered episode after episode (ACT's ticks, a progress
    model's intervals): whole episodes held out with the run's seed."""

    train: np.ndarray
    validation: np.ndarray

    @classmethod
    def choose(cls, episode_items: list[int], fraction: float, seed: int) -> EpisodeSplit:
        """`episode_items[e]` is how many items episode `e` has."""
        episodes = len(episode_items)
        rng = np.random.default_rng(seed)
        held_out = set(rng.choice(episodes, size=cls.held_out(episodes, fraction), replace=False))
        offsets = np.cumsum([0, *episode_items])
        spans = [np.arange(offsets[index], offsets[index + 1]) for index in range(episodes)]
        return cls(
            train=np.concatenate([span for i, span in enumerate(spans) if i not in held_out]),
            validation=np.concatenate(
                [np.empty(0, np.int64)] + [span for i, span in enumerate(spans) if i in held_out]
            ),
        )

    @staticmethod
    def held_out(episodes: int, fraction: float) -> int:
        """`round(fraction * episodes)` (halves up), at least one when the fraction is above zero
        and there are two or more episodes, and never every episode."""
        count = math.floor(fraction * episodes + 0.5)
        if fraction > 0 and episodes >= 2:
            count = max(count, 1)
        return min(count, episodes - 1)
