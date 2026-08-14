# Spiral training runtime

`training/` owns the complete remote model-compute boundary:

- `contracts/` is the framework-neutral Rust interface used by Studio;
- `http-client/` is Studio's JSON + Arrow IPC client;
- `src/spiral_training_runtime/` is the Python HTTP service;
- `uv.lock` is the reproducible local, CI, and Lambda dependency definition.

The service uses Hugging Face LeRobot's Apache-2.0 ACT implementation directly
through PyTorch. There is no LibTorch or model implementation in the Rust
workspace.

## Lambda Cloud: clone and run

Choose a Lambda Stack 24.04 image. Lambda Stack already includes the
NVIDIA driver and CUDA runtime. The bootstrap installs `uv` locally when it is
not already available, syncs the locked Python 3.12 environment, verifies CUDA,
and starts the service. Linux resolves PyTorch and torchvision from the official
CUDA 12.8 wheel index declared in `pyproject.toml`; no global Python packages or
container image are required.

```sh
git clone https://github.com/spiraldb/spiral-robotics.git
cd spiral-robotics
./training/bootstrap-lambda.sh
```

The script verifies Python, creates `training/.venv`, installs the complete
locked runtime, verifies that PyTorch can see the GPU, and serves on
`0.0.0.0:50051`. It intentionally stays in the foreground so SSH, `tmux`,
`systemd`, or the cloud-init caller owns process lifetime and logs.

Connect Studio to:

```text
http://LAMBDA_INSTANCE_IP:50051
```

The current prototype has no authentication or TLS. Bind it only on a trusted
network or use an SSH tunnel. Set `SPIRAL_RUNTIME_PORT` to change the port and
`SPIRAL_POLICY_DEVICE` to select the rollout device; both default to automatic
CUDA selection.

## Local development

```sh
cd training
uv sync --extra dev
uv run pytest
uv run ruff check .
uv run ruff format --check .
uv run spiral-training-runtime --host 127.0.0.1 --port 50051
```

## Stable protocol

Control plane:

- `GET /v1/health`
- `POST /v1/sessions`
- `DELETE /v1/sessions/{id}`
- `GET /v1/sessions/{id}/telemetry`
- `POST /v1/sessions/{id}/checkpoint`
- `POST /v1/policies`
- `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`

Data plane:

- `POST /v1/sessions/{id}/train?step=N`
- `POST /v1/sessions/{id}/validate`
- `POST /v1/policies/{id}/infer`

Data-plane bodies are one `application/vnd.apache.arrow.stream` record batch.
Tensor columns are non-null `FixedSizeList` arrays. PyArrow exposes their child
buffers as zero-copy NumPy views; `torch.from_numpy` shares the CPU storage.
Moving a batch to CUDA necessarily performs the host-to-device transfer.

Checkpoints are raw SafeTensors bytes with format metadata in
`x-spiral-checkpoint-format` and `x-spiral-checkpoint-extension` headers.
The runtime is stateless beyond in-memory training and policy sessions; Studio
continues to own datasets, run history, and durable artifact publication.
