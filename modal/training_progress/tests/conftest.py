from __future__ import annotations

from pathlib import Path

import pytest
from warp_progress.testing import StandInBackbone

from spiral_training_runtime.testing import LocalSync
from spiral_training_runtime.volume import TrainingVolume


@pytest.fixture
def volume(tmp_path: Path) -> TrainingVolume:
    return TrainingVolume(tmp_path / "volume", LocalSync())


@pytest.fixture(scope="session")
def backbone(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A stand-in for the gated DINOv3 ViT-B/16: one layer with the real one's 768-wide CLS, laid
    out as the pinned one is."""
    directory = tmp_path_factory.mktemp("dinov3")
    StandInBackbone.save(directory)
    return directory
