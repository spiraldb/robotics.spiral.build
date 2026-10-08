"""Publishing a progress model: one progress checkpoint of a run copied into the Modal Volume
`spiral-robotics-models` under the SHA-256 of its weights, where Score progress loads it by that
hash. Only the control endpoint writes that Volume. See docs/training.md.

    models/<sha256>/model.safetensors
    models/<sha256>/config.json
"""

from __future__ import annotations

import hashlib
import json
import shutil
import threading
from pathlib import Path
from typing import Any

from .errors import RuntimeRequestError
from .request import RunKey, VolumeName, WireModel
from .volume import TrainingVolume, VolumeSync

MODELS_VOLUME = "spiral-robotics-models"
WEIGHTS = "model.safetensors"
MANIFEST = "config.json"


class ModelPublishRequest(WireModel):
    """Which checkpoint to publish, as Studio posts it: `model-publish-request.json`."""

    project: VolumeName
    run_id: VolumeName
    step: int

    @property
    def key(self) -> RunKey:
        return RunKey(self.project, self.run_id)


class PublishedModel(WireModel):
    """A published model as Score progress is asked for it, and its checkpoint's manifest:
    `model-publish.json`."""

    model: str
    manifest: dict[str, Any]


class ModelPublisher:
    """Copies runs' progress checkpoints from the training Volume into the models Volume, mounted at
    `models`, and commits them with `sync`."""

    def __init__(self, runs: TrainingVolume, models: Path, sync: VolumeSync) -> None:
        self._runs = runs
        self._models = models / "models"
        self._sync = sync
        # Publishes take turns, so two of one checkpoint copy it once and a commit never meets a
        # file open on the mount.
        self._lock = threading.Lock()

    def publish(self, request: ModelPublishRequest) -> tuple[PublishedModel, bool]:
        """The checkpoint as published, and whether this call published it rather than finding it
        already there."""
        key, step = request.key, request.step
        with self._runs.reading():
            files = self._runs.run(key)
            if not files.exists():
                raise RuntimeRequestError(f"run {key.run_id} is unknown", 404)
            checkpoint = files.checkpoint(step)
            if not all((checkpoint / name).is_file() for name in (WEIGHTS, MANIFEST)):
                raise RuntimeRequestError(f"run {key.run_id} has no checkpoint {step}", 404)
            manifest = json.loads((checkpoint / MANIFEST).read_bytes())
            if manifest.get("family") != "progress":
                raise RuntimeRequestError(
                    f"checkpoint {step} of run {key.run_id} is not a progress model; only those "
                    "are published",
                    409,
                )
            digest, created = self._copy(checkpoint)
        return PublishedModel(model=f"{MODELS_VOLUME}/{digest}", manifest=manifest), created

    def _copy(self, checkpoint: Path) -> tuple[str, bool]:
        """Copies a checkpoint's files to `models/<sha256>/` unless they are there already: the hash
        and whether this call copied them. They are copied beside it and renamed into place, so a
        reader never sees part of a model: Modal also commits in the background, at any moment."""
        with (checkpoint / WEIGHTS).open("rb") as weights:
            digest = hashlib.file_digest(weights, "sha256").hexdigest()
        published = self._models / digest
        with self._lock:
            if published.is_dir():
                return digest, False
            partial = published.with_name(f"{digest}.partial")
            # Left by a container that went away mid-copy.
            shutil.rmtree(partial, ignore_errors=True)
            partial.mkdir(parents=True)
            for name in (WEIGHTS, MANIFEST):
                shutil.copyfile(checkpoint / name, partial / name)
            partial.rename(published)
            self._sync.commit()
        return digest, True
