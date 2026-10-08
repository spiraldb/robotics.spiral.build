"""The progress model's training windows, drawn over its camera's embeddings as upstream WARP-RM
draws them over its cached features, in worker processes ahead of the training loop. See
docs/training.md.
"""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import torch
from torch.utils.data import DataLoader, IterableDataset, get_worker_info

from .progress_recipe import EMBEDDING_SIZE, WINDOW_SIZE, SimRecipe
from .request import Optimization
from .validation import EpisodeSplit
from .warp_rm_upstream.data.progress_curves import uniform_progress


@dataclass(frozen=True)
class EmbeddedInterval:
    """One kept interval's pictures: `frames` rows of the embeddings from `first_row`, in display
    order."""

    episode: int
    first_row: int
    frames: int


class EmbeddedCamera:
    """A run's camera embedded on the job's local disk: float32 rows, one per picture, episode after
    episode, and the kept intervals over them.

    It is memory-mapped wherever it is opened: a sampling worker unpickles the file's path and maps
    the same file, sharing the page cache rather than a copy.
    """

    def __init__(self, path: Path, intervals: list[EmbeddedInterval], episodes: int) -> None:
        self._path = path
        self._intervals = intervals
        self._episodes = episodes
        rows = path.stat().st_size // (EMBEDDING_SIZE * np.dtype(np.float32).itemsize)
        self._rows = np.memmap(path, dtype=np.float32, mode="r", shape=(rows, EMBEDDING_SIZE))

    def __reduce__(self) -> tuple[type[EmbeddedCamera], tuple[Path, list[EmbeddedInterval], int]]:
        return (EmbeddedCamera, (self._path, self._intervals, self._episodes))

    @property
    def episodes(self) -> int:
        return self._episodes

    @property
    def intervals(self) -> list[EmbeddedInterval]:
        """The kept intervals that hold a picture, episode by episode."""
        return self._intervals

    @property
    def frames(self) -> int:
        return sum(interval.frames for interval in self._intervals)

    def rows(self, indices: np.ndarray) -> np.ndarray:
        """The embeddings at `indices`, shaped as `indices` plus the embedding."""
        return np.asarray(self._rows[indices])


@dataclass(frozen=True)
class WindowBatch:
    """Training windows: their embeddings and their relative progress labels."""

    features: torch.Tensor  # [windows, 32, 768] float32
    labels: torch.Tensor  # [windows, 32] float32

    @property
    def size(self) -> int:
        return len(self.labels)

    def pin_memory(self) -> WindowBatch:
        """The batch in page-locked memory, where a DataLoader on CUDA puts it, so its copy to the
        device runs asynchronously."""
        return WindowBatch(self.features.pin_memory(), self.labels.pin_memory())


class ProgressWindows:
    """Windows over the embedded intervals, each drawn as upstream's `PrecomputedFeatureDataset`
    draws one: an interval, the AR sampler's 32 indices into it, and their relative cumulative
    labels against the interval's uniform progress. Whole episodes are held out for validation with
    the run's seed. Training batches are built in `workers` processes."""

    def __init__(self, camera: EmbeddedCamera, optimization: Optimization, workers: int) -> None:
        self._camera = camera
        self._workers = workers
        per_episode = np.bincount(
            [interval.episode for interval in camera.intervals], minlength=camera.episodes
        )
        self._split = EpisodeSplit.choose(
            per_episode.tolist(), optimization.validation_split, optimization.seed
        )
        if not len(self._split.train):
            raise ValueError("no training episode has a kept interval that holds a picture")
        self._sampler = SimRecipe.sampler()
        self._labeler = SimRecipe.labeler()
        # Upstream's `--progress-shape uniform`: progress(f) = f / frames.
        self._progress = [uniform_progress(interval.frames) for interval in camera.intervals]

    @property
    def has_validation(self) -> bool:
        return len(self._split.validation) > 0

    def train_batches(self, seed: np.random.SeedSequence, size: int) -> Iterator[WindowBatch]:
        """Batches from the worker processes, round-robin, each drawing from its own child of
        `seed`. The parent has initialized CUDA, so workers fork from a fresh forkserver process
        rather than from it; spawned workers would take seconds each to shut down."""
        loader = DataLoader(
            WindowStream(self, seed, size),
            batch_size=None,
            num_workers=self._workers,
            multiprocessing_context="forkserver",
            pin_memory=torch.cuda.is_available(),
        )
        yield from loader

    def train_batch(self, rng: np.random.Generator, size: int) -> WindowBatch:
        return self.windows(rng.choice(self._split.train, size))

    def validation_batch(self, rng: np.random.Generator, size: int) -> WindowBatch:
        return self.windows(rng.choice(self._split.validation, size))

    def windows(self, intervals: np.ndarray) -> WindowBatch:
        """One window drawn from each of `intervals`, indices into `EmbeddedCamera.intervals`."""
        rows = np.empty((len(intervals), WINDOW_SIZE), np.int64)
        labels = np.empty((len(intervals), WINDOW_SIZE), np.float32)
        for window, index in enumerate(intervals):
            interval = self._camera.intervals[index]
            indices = self._sampler.sample_indices(interval.frames)
            labels[window] = self._labeler.generate(indices, self._progress[index], interval.frames)
            rows[window] = interval.first_row + np.asarray(indices)
        features = torch.from_numpy(self._camera.rows(rows))
        return WindowBatch(features, torch.from_numpy(labels))


class WindowStream(IterableDataset):
    """Training batches without end, as one DataLoader worker process draws them."""

    def __init__(self, windows: ProgressWindows, seed: np.random.SeedSequence, size: int) -> None:
        self._windows = windows
        self._seed = seed
        self._size = size

    def __iter__(self) -> Iterator[WindowBatch]:
        worker = get_worker_info()
        rng = np.random.default_rng(self._seed.spawn(worker.num_workers)[worker.id])
        while True:
            yield self._windows.train_batch(rng, self._size)
