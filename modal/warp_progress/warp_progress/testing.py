"""What the tests of warp-progress, and of the training job that embeds with it, run on: stand-ins for
the gated DINOv3 backbone and for a temporal head, and a local directory standing in for the models
Volume."""

import hashlib
import json
from pathlib import Path

import torch
from transformers import DINOv3ViTConfig, DINOv3ViTModel

from warp_progress.warp import TemporalModel


class StandInBackbone:
    """A stand-in for the gated DINOv3 ViT-B/16: one layer with the real one's 768-wide CLS."""

    @staticmethod
    def save(directory: Path) -> None:
        """Writes it to `directory`, laid out as the pinned one is."""
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


class StandInHead:
    """A temporal head of random weights, its bins centred over ±3 as the released head's are."""

    @staticmethod
    def tensors(seed: int) -> dict[str, torch.Tensor]:
        torch.manual_seed(seed)
        head = TemporalModel()
        torch.nn.init.normal_(head.pos_embed, std=0.02)
        head.rel_bin_centers.copy_(torch.linspace(-3.0, 3.0, 30))
        return head.state_dict()


class LocalModelsVolume:
    """A local directory standing in for the models Volume: it changes only when a test publishes
    to it, and counts the reloads asked of it."""

    def __init__(self, root: Path):
        self.root = root
        self.reloads = 0

    def reload(self) -> None:
        self.reloads += 1

    def publish(self, weights: bytes, manifest: dict) -> str:
        """Lays out a progress model as the training app publishes it, and answers its name."""
        digest = hashlib.sha256(weights).hexdigest()
        directory = self.root / "models" / digest
        directory.mkdir(parents=True)
        (directory / "model.safetensors").write_bytes(weights)
        (directory / "config.json").write_text(json.dumps(manifest))
        return f"spiral-robotics-models/{digest}"
