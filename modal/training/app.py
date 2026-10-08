"""Modal deployment of Spiral Robotics training: a CPU control endpoint in front of one GPU function
per model family, ACT on an L4 and the progress model on an H100, sharing one Volume. See
docs/training.md#the-modal-service.

    MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
"""

import os
from pathlib import Path

import modal

from spiral_training_runtime.control import GPU, PROGRESS_GPU, ControlService, RunControl
from spiral_training_runtime.request import ModelKind
from spiral_training_runtime.volume import TrainingVolume

VOLUME_ROOT = Path("/training")
# Where the progress image holds Score progress's DINOv3 backbone.
ARTIFACTS = Path("/artifacts")
# The longest a Modal function may run.
TIMEOUT_S = 24 * 3600
# The progress job's cores: the training loop keeps two, and the rest build its batches.
PROGRESS_CPUS = 8
TRAINING = Path(__file__).resolve().parent
# What `add_local_python_source` leaves out of a package, for the packages added by path.
NOT_PYTHON = ~modal.FilePatternMatcher("**/*.py")


def download_backbone() -> None:
    """Progress image build step: DINOv3 is gated, so the build fetches it with a Hugging Face token
    and running containers never need one."""
    from warp_progress.artifacts import Artifacts

    Artifacts(ARTIFACTS).download_backbone()


image = (
    modal.Image.debian_slim(python_version="3.12")
    .uv_sync()
    .add_local_python_source("spiral_training_runtime")
)
# The progress job's own environment, Score progress's stack: see
# ../training_progress/pyproject.toml. Its path dependencies are not installed where this deploys
# from, so their sources are added by path. The build step imports this file, so the sources it
# imports are copied in before it: changing any of them fetches the backbone again on the next
# deploy.
progress_image = (
    modal.Image.debian_slim(python_version="3.12")
    .uv_sync(
        str(TRAINING.parent / "training_progress"),
        extra_options="--no-dev "
        "--no-install-package warp-progress --no-install-package video-service",
    )
    .add_local_python_source("spiral_training_runtime", copy=True)
    .add_local_dir(
        TRAINING.parent / "warp_progress" / "warp_progress",
        "/root/warp_progress",
        copy=True,
        ignore=NOT_PYTHON,
    )
    .add_local_dir(
        TRAINING.parent / "video_service" / "video_service",
        "/root/video_service",
        copy=True,
        ignore=NOT_PYTHON,
    )
    .run_function(
        download_backbone, secrets=[modal.Secret.from_name("spiral-robotics-huggingface")]
    )
)
app = modal.App("spiral-robotics-training", image=image)
volume = modal.Volume.from_name("spiral-robotics-training", create_if_missing=True)
stop_flags = modal.Dict.from_name("spiral-robotics-training-stops", create_if_missing=True)
job_options = {
    "memory": 32 * 1024,
    "timeout": TIMEOUT_S,
    "retries": modal.Retries(max_retries=3, initial_delay=10.0),
    "volumes": {VOLUME_ROOT: volume},
}


@app.function(gpu=GPU, cpu=8, **job_options)
def train(request_json: str) -> None:
    """An ACT run's job."""
    # Imported here so the control container never loads PyTorch.
    from spiral_training_runtime.act import ActTraining
    from spiral_training_runtime.request import ActRunRequest
    from spiral_training_runtime.run import TrainingJob

    training_volume = TrainingVolume(VOLUME_ROOT, volume)
    training = ActTraining(ActRunRequest.model_validate_json(request_json), training_volume)
    TrainingJob(training_volume, stop_flags, training).run()


@app.function(gpu=PROGRESS_GPU, image=progress_image, cpu=PROGRESS_CPUS, **job_options)
def train_progress(request_json: str) -> None:
    """A progress model run's job."""
    from warp_progress.artifacts import Artifacts

    from spiral_training_runtime.progress import ProgressResources, ProgressTraining
    from spiral_training_runtime.request import ProgressRunRequest
    from spiral_training_runtime.run import TrainingJob

    training_volume = TrainingVolume(VOLUME_ROOT, volume)
    resources = ProgressResources(Artifacts(ARTIFACTS).backbone, PROGRESS_CPUS - 2)
    request = ProgressRunRequest.model_validate_json(request_json)
    training = ProgressTraining(request, training_volume, resources)
    TrainingJob(training_volume, stop_flags, training).run()


class ModalCalls:
    """The GPU functions' calls, by ID."""

    def spawn(self, kind: ModelKind, request_json: str) -> str:
        function = {"act": train, "progress": train_progress}[kind]
        return function.spawn(request_json).object_id

    def ended(self, call_id: str) -> bool:
        try:
            modal.FunctionCall.from_id(call_id).get(timeout=0)
        except TimeoutError:
            # Still running, or waiting for a retry.
            return False
        except Exception:
            # Failed, cancelled, or so long gone that its result expired.
            return True
        return True

    def cancel(self, call_id: str) -> None:
        modal.FunctionCall.from_id(call_id).cancel()


# One container owns the control side of the Volume, so its reloads and commits never race
# another control container's.
@app.function(
    secrets=[modal.Secret.from_name("spiral-robotics-auth")],
    volumes={VOLUME_ROOT: volume},
    max_containers=1,
)
@modal.concurrent(max_inputs=32)
@modal.asgi_app()
def control():
    training_volume = TrainingVolume(VOLUME_ROOT, volume)
    runs = RunControl(training_volume, ModalCalls(), stop_flags)
    return ControlService(training_volume, runs, os.environ["MODAL_SERVICE_TOKEN"]).app()
