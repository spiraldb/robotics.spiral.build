"""The prepare step: the training clock laid over every kept interval, and each camera's frame and
each joint stream's value sampled at every tick. See docs/training.md#how-the-model-sees-the-data.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from .request import RunRequest
from .streams import CameraStream, ValueStream
from .video import CameraVideo
from .volume import TrainingVolume

STD_FLOOR = 1.0e-3
NANOSECONDS = 1_000_000_000


@dataclass(frozen=True)
class EpisodeTicks:
    """The training clock over one episode's kept intervals, interval after interval."""

    times_ns: np.ndarray
    # For each tick, the index one past its interval's last tick.
    interval_ends: np.ndarray

    @classmethod
    def over(cls, intervals: list[tuple[int, int]], rate_hz: float) -> EpisodeTicks:
        """Ticks `start + k / rate` for every `k` whose tick is before the interval's `end`."""
        times: list[np.ndarray] = []
        ends: list[np.ndarray] = []
        count = 0
        for start, end in intervals:
            candidates = np.arange(math.ceil((end - start) * rate_hz / NANOSECONDS) + 1)
            ticks = start + np.round(candidates * (NANOSECONDS / rate_hz)).astype(np.int64)
            ticks = ticks[ticks < end]
            count += len(ticks)
            times.append(ticks)
            ends.append(np.full(len(ticks), count, np.int64))
        return cls(np.concatenate(times), np.concatenate(ends))

    def __len__(self) -> int:
        return len(self.times_ns)


@dataclass(frozen=True)
class EpisodeSplit:
    """Which ticks train and which validate: whole episodes held out with the run's seed."""

    train: np.ndarray
    validation: np.ndarray

    @classmethod
    def choose(cls, episode_ticks: list[int], fraction: float, seed: int) -> EpisodeSplit:
        episodes = len(episode_ticks)
        rng = np.random.default_rng(seed)
        held_out = set(rng.choice(episodes, size=cls.held_out(episodes, fraction), replace=False))
        offsets = np.cumsum([0, *episode_ticks])
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


@dataclass(frozen=True)
class Normalization:
    """State and action mean and standard deviation over the training ticks."""

    state_mean: np.ndarray
    state_std: np.ndarray
    action_mean: np.ndarray
    action_std: np.ndarray

    @classmethod
    def over(cls, states: np.ndarray, actions: np.ndarray) -> Normalization:
        return cls(
            state_mean=states.mean(axis=0).astype(np.float32),
            state_std=np.maximum(states.std(axis=0), STD_FLOOR).astype(np.float32),
            action_mean=actions.mean(axis=0).astype(np.float32),
            action_std=np.maximum(actions.std(axis=0), STD_FLOOR).astype(np.float32),
        )


@dataclass(frozen=True)
class TrainingBatch:
    """Model inputs for some ticks: normalized values and HWC RGB bytes."""

    states: np.ndarray  # [batch, state] float32
    images: np.ndarray  # [batch, camera, height, width, 3] uint8
    actions: np.ndarray  # [batch, chunk, action] float32
    action_is_pad: np.ndarray  # [batch, chunk] bool

    @property
    def size(self) -> int:
        return len(self.states)


class TrainingSamples:
    """Every tick of a run's kept intervals, sampled: frames in a uint8 memmap on local disk, the
    normalized state and action in memory."""

    def __init__(self, request: RunRequest, volume: TrainingVolume, directory: Path) -> None:
        observation = request.config.observation
        optimization = request.config.optimization
        clocks = [
            EpisodeTicks.over(episode.intervals, observation.rate_hz)
            for episode in request.episodes
        ]
        ticks = sum(map(len, clocks))
        image = observation.image
        self._frames = np.memmap(
            directory / "frames.u8",
            dtype=np.uint8,
            mode="w+",
            shape=(ticks, len(observation.cameras), image.height, image.width, 3),
        )
        states = np.empty((ticks, len(request.joints.state)))
        actions = np.empty((ticks, len(request.joints.action)))
        self._interval_ends = np.empty(ticks, np.int64)
        offset = 0
        for episode, clock in zip(request.episodes, clocks, strict=True):
            span = slice(offset, offset + len(clock))
            try:
                for camera, key in enumerate(episode.cameras):
                    video = CameraVideo(CameraStream.read(volume.stream(key)))
                    video.sample(clock.times_ns, self._frames[span, camera])
                states[span] = self._held_values(volume, episode.state, clock, request.joints.state)
                actions[span] = self._held_values(
                    volume, episode.action, clock, request.joints.action
                )
            except Exception as error:
                raise ValueError(f"episode {episode.episode_id}: {error}") from error
            self._interval_ends[span] = clock.interval_ends + offset
            offset += len(clock)
        self._split = EpisodeSplit.choose(
            [len(clock) for clock in clocks], optimization.validation_split, optimization.seed
        )
        self._normalization = Normalization.over(
            states[self._split.train], actions[self._split.train]
        )
        stats = self._normalization
        self._states = ((states - stats.state_mean) / stats.state_std).astype(np.float32)
        self._actions = ((actions - stats.action_mean) / stats.action_std).astype(np.float32)
        self._chunk_size = optimization.chunk_size

    @property
    def ticks(self) -> int:
        return len(self._interval_ends)

    @property
    def normalization(self) -> Normalization:
        return self._normalization

    @property
    def has_validation(self) -> bool:
        return len(self._split.validation) > 0

    def train_batch(self, rng: np.random.Generator, size: int) -> TrainingBatch:
        return self.batch(rng.choice(self._split.train, size))

    def validation_batch(self, rng: np.random.Generator, size: int) -> TrainingBatch:
        return self.batch(rng.choice(self._split.validation, size))

    def batch(self, ticks: np.ndarray) -> TrainingBatch:
        """Each tick's observation and the actions at it and its next `chunk_size - 1` ticks;
        ticks past its interval's end are padded with the interval's last action."""
        chunk = ticks[:, None] + np.arange(self._chunk_size)
        interval_ends = self._interval_ends[ticks][:, None]
        return TrainingBatch(
            states=self._states[ticks],
            images=np.asarray(self._frames[ticks]),
            actions=self._actions[np.minimum(chunk, interval_ends - 1)],
            action_is_pad=chunk >= interval_ends,
        )

    @staticmethod
    def _held_values(
        volume: TrainingVolume, keys: list[str], clock: EpisodeTicks, joints: list[str]
    ) -> np.ndarray:
        """The state or the action at every tick: its streams' held values side by side."""
        streams = [ValueStream.read(volume.stream(key)) for key in keys]
        values = np.column_stack([stream.held(clock.times_ns) for stream in streams])
        if values.shape[1] != len(joints):
            raise ValueError(f"streams carry {values.shape[1]} values for the joints {joints}")
        return values
