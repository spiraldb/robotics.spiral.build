"""Modal deployment of the WARP-RM progress service on T4s, with the checkpoint baked into the image.

    modal deploy app.py
"""

import logging
import os
from pathlib import Path

import modal

ARTIFACTS = Path("/artifacts")
# Modal streams each request body into its container, so a slow upload holds an input slot while
# the GPU idles. At 0.2 MB/s per request (33 Mbit/s shared by 20), a T4 in fp16 is 32% busy with 4
# requests, 61% with 8 and 84% with 12; 20 requests in flight then spread over about 3 containers.
MAX_INPUTS = 12
TARGET_INPUTS = 8
# A request's time includes its upload: a 10,000-frame episode (about 80 MB) takes about 400 s at
# 0.2 MB/s, and up to 11 others can be ahead of it for the GPU.
TIMEOUT_S = 1800


def download_artifacts() -> None:
    """Image build step, so containers start with the weights and never need a Hugging Face token."""
    from warp_progress.artifacts import Artifacts

    Artifacts(ARTIFACTS).download()


image = (
    modal.Image.debian_slim(python_version="3.12")
    .uv_sync(extra_options="--no-dev")
    .add_local_python_source("warp_progress", copy=True)
    .run_function(download_artifacts, secrets=[modal.Secret.from_name("spiral-robotics-huggingface")])
)
app = modal.App("warp-progress", image=image)

with image.imports():
    import torch

    from warp_progress.artifacts import Artifacts
    from warp_progress.service import ProgressService
    from warp_progress.warp import WarpModel


@app.cls(
    gpu="T4",
    secrets=[modal.Secret.from_name("spiral-robotics-auth")],
    scaledown_window=120,
    min_containers=0,
    timeout=TIMEOUT_S,
)
# Requests upload and decode while another's batch runs on the GPU, which WarpModel serves one
# batch at a time.
@modal.concurrent(max_inputs=MAX_INPUTS, target_inputs=TARGET_INPUTS)
class WarpProgress:
    @modal.enter()
    def load(self) -> None:
        logging.basicConfig(level=logging.INFO)
        self._model = WarpModel(Artifacts(ARTIFACTS), torch.device("cuda"))

    @modal.asgi_app()
    def web(self):
        return ProgressService(self._model, os.environ["MODAL_SERVICE_TOKEN"]).app()
