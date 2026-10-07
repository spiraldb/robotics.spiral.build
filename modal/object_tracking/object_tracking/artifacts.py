"""The pinned SAM 3.1 checkpoint the tracker loads."""

from pathlib import Path

from huggingface_hub import hf_hub_download

SELECTOR = "facebook/sam3.1@daa63191845a41281374e725f4c9e51c7a824460"
REPOSITORY, REVISION = SELECTOR.split("@")
CHECKPOINT = "sam3.1_multiplex.pt"


class Artifacts:
    """The released SAM 3.1 multiplex checkpoint under one directory."""

    def __init__(self, root: Path):
        self._root = root

    @property
    def checkpoint(self) -> Path:
        return self._root / CHECKPOINT

    def download(self) -> None:
        """Fetch the checkpoint at its pinned revision; SAM 3.1 is gated, so `HF_TOKEN` must be set
        for an account Meta has granted access."""
        hf_hub_download(REPOSITORY, CHECKPOINT, revision=REVISION, local_dir=self._root)
