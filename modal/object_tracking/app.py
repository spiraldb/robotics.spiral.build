"""Modal deployment of the object tracking service on H100s, with the checkpoint baked into the image.

    MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
"""

import logging
import os
from pathlib import Path

import modal

ARTIFACTS = Path("/artifacts")
# Two workers track at a time while the other requests upload and decode, so the GPU never waits
# for an episode to be prepared. Studio sends up to eight episodes at once, two containers' worth.
MAX_INPUTS = 6
TARGET_INPUTS = 4
# A request's time includes its upload and the episodes ahead of it.
TIMEOUT_S = 1800


def download_artifacts() -> None:
    """Image build step, so containers start with the weights and never need a Hugging Face token."""
    from object_tracking.artifacts import Artifacts

    Artifacts(ARTIFACTS).download()


image = (
    modal.Image.debian_slim(python_version="3.12")
    # uv fetches SAM 3 from its Git repository.
    .apt_install("git", "libgl1", "libglib2.0-0")
    # The shared package is a path dependency outside the image; its source is copied in instead.
    .uv_sync(extra_options="--no-dev --no-install-package video-service")
    .add_local_python_source("video_service", "object_tracking", copy=True)
    .run_function(download_artifacts, secrets=[modal.Secret.from_name("spiral-robotics-huggingface")])
)
app = modal.App("spiral-robotics-object-tracking", image=image)

with image.imports():
    from object_tracking.artifacts import Artifacts
    from object_tracking.service import TrackingService
    from object_tracking.tracker import TrackerPool


@app.cls(
    gpu="H100",
    cpu=8,
    memory=32 * 1024,
    secrets=[modal.Secret.from_name("spiral-robotics-auth")],
    scaledown_window=120,
    min_containers=0,
    timeout=TIMEOUT_S,
)
@modal.concurrent(max_inputs=MAX_INPUTS, target_inputs=TARGET_INPUTS)
class ObjectTracking:
    @modal.enter()
    def load(self) -> None:
        logging.basicConfig(level=logging.INFO)
        self._pool = TrackerPool.start(Artifacts(ARTIFACTS).checkpoint)

    @modal.exit()
    def close(self) -> None:
        self._pool.close()

    @modal.asgi_app()
    def web(self):
        return TrackingService(self._pool, os.environ["MODAL_SERVICE_TOKEN"]).app()
