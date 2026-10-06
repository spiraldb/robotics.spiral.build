"""Modal deployment of Spiral Robotics training: a CPU control endpoint in front of an L4 job,
sharing one Volume. See docs/training.md#the-modal-service.

    uv run modal deploy app.py
"""

import os
from pathlib import Path

import modal

from spiral_training_runtime.control import GPU, ControlService, RunControl
from spiral_training_runtime.volume import TrainingVolume

VOLUME_ROOT = Path("/training")
# The longest a Modal function may run.
TIMEOUT_S = 24 * 3600

image = (
    modal.Image.debian_slim(python_version="3.12")
    .uv_sync()
    .add_local_python_source("spiral_training_runtime")
)
app = modal.App("spiral-robotics-training", image=image)
volume = modal.Volume.from_name("spiral-robotics-training", create_if_missing=True)
stop_flags = modal.Dict.from_name("spiral-robotics-training-stops", create_if_missing=True)


@app.function(
    gpu=GPU,
    cpu=8,
    memory=32 * 1024,
    timeout=TIMEOUT_S,
    retries=modal.Retries(max_retries=3, initial_delay=10.0),
    volumes={VOLUME_ROOT: volume},
)
def train(request_json: str) -> None:
    # Imported here so the control container never loads PyTorch.
    from spiral_training_runtime.run import TrainingJob

    TrainingJob(TrainingVolume(VOLUME_ROOT, volume), stop_flags, request_json).run()


class ModalCalls:
    """The `train` function's calls, by ID."""

    def spawn(self, request_json: str) -> str:
        return train.spawn(request_json).object_id

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
