"""The pinned checkpoint files `spql.ai.warp_rm` reads, laid out as its Rust reference test expects."""

from pathlib import Path

from huggingface_hub import hf_hub_download

SELECTOR = "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
BACKBONE_REPOSITORY = "facebook/dinov3-vitb16-pretrain-lvd1689m"
BACKBONE_REVISION = "5931719e67bbdb9737e363e781fb0c67687896bc"
HEAD_REPOSITORY, HEAD_REVISION = SELECTOR.split("@")
HEAD_FILE = "warp_rm_sss15.pt"


class Artifacts:
    """The official DINOv3 ViT-B/16 backbone and the WARP-RM sim-bottles head under one directory."""

    def __init__(self, root: Path):
        self._root = root

    @property
    def backbone(self) -> Path:
        """The directory holding the backbone's `config.json` and `model.safetensors`."""
        return self._root / "dinov3-vitb16"

    @property
    def head(self) -> Path:
        return self._root / "warp-rm-sim-bottles-sss15" / HEAD_FILE

    def download(self) -> None:
        """Fetch every file at its pinned revision; DINOv3 is gated, so `HF_TOKEN` must be set."""
        self.download_backbone()
        hf_hub_download(
            HEAD_REPOSITORY, HEAD_FILE, revision=HEAD_REVISION, local_dir=self.head.parent
        )

    def download_backbone(self) -> None:
        """Fetch the backbone alone at its pinned revision, as the training job's image does; DINOv3 is
        gated, so `HF_TOKEN` must be set."""
        for name in ("config.json", "model.safetensors"):
            hf_hub_download(
                BACKBONE_REPOSITORY, name, revision=BACKBONE_REVISION, local_dir=self.backbone
            )
