"""Published progress models: progress checkpoints that the training app's control endpoint copied
into the Modal Volume `spiral-robotics-models` under the SHA-256 of their weights. A request names
one as `spiral-robotics-models/<sha256>`. See docs/training.md.

    models/<sha256>/model.safetensors    the head's tensors under upstream `TransformerAggregator` names
    models/<sha256>/config.json          its manifest, `spiral_robotics.progress.safetensors.v1`
"""

import json
import re
from pathlib import Path
from typing import Protocol

import torch
from safetensors.torch import load as load_safetensors
from video_service.service import IncompatibleModel, UnknownModel

from warp_progress.artifacts import BACKBONE_REPOSITORY, BACKBONE_REVISION, SELECTOR

PUBLISHED_MODEL = re.compile(r"spiral-robotics-models/([0-9a-f]{64})")
# What a published model's manifest must say for this scorer to score with it: pictures embedded,
# windowed and binned as this scorer embeds, windows and reads them.
SCORED_MANIFEST = {
    "family": "progress",
    "manifest": "spiral_robotics.progress.safetensors.v1",
    "backbone": f"{BACKBONE_REPOSITORY}@{BACKBONE_REVISION}",
    "window_size": 32,
    "feature_stride": 1,
    "standard_stride_src": 15,
    "fps": 30,
    "bins": 30,
    "bin_range": [-3.0, 3.0],
    "ablation": "no_abs",
    "image": {"width": 224, "height": 224},
    "crop": "squash",
}


class ModelsVolume(Protocol):
    """How a mount of the models Volume sees what was published after it was mounted; a
    `modal.Volume` is one."""

    def reload(self) -> None: ...


class PublishedModels:
    """The published progress models on a read-only mount of their Volume, at `root`."""

    def __init__(self, root: Path, volume: ModelsVolume):
        self._root = root
        self._volume = volume

    def tensors(self, model: str | None) -> dict[str, torch.Tensor]:
        """The head tensors of the published `model`, read after a reload so that a model published
        since the mount was made is found. One call at a time: Modal fails a reload while any file on
        the mount is open."""
        match = PUBLISHED_MODEL.fullmatch(model or "")
        if match is None:
            raise UnknownModel(
                f"unknown model {model!r}; expected {SELECTOR!r} or spiral-robotics-models/<sha256>"
            )
        self._volume.reload()
        directory = self._root / "models" / match[1]
        if not directory.is_dir():
            raise UnknownModel(f"no progress model {model} is published")
        manifest = json.loads((directory / "config.json").read_bytes())
        mismatches = [
            f"its {key} is {manifest.get(key)!r}, this scorer's {expected!r}"
            for key, expected in SCORED_MANIFEST.items()
            if manifest.get(key) != expected
        ]
        if mismatches:
            raise IncompatibleModel(f"published model {model} cannot be scored here: {'; '.join(mismatches)}")
        return load_safetensors((directory / "model.safetensors").read_bytes())
