from __future__ import annotations

from pathlib import Path

import pytest
import torch
from transformers import DINOv3ViTConfig, DINOv3ViTModel

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
    torch.manual_seed(0)
    config = DINOv3ViTConfig(
        hidden_size=768,
        num_hidden_layers=1,
        num_attention_heads=12,
        intermediate_size=64,
        patch_size=56,
        num_register_tokens=0,
    )
    DINOv3ViTModel(config).save_pretrained(directory)
    return directory
