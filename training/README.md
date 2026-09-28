# Spiral training runtime

`training/` owns the complete remote training-worker boundary:

- `../crates/backend/training/` is the framework-neutral protocol used
  by Studio and the worker;
- `../crates/backend/src/adapters/compute_runtime.rs` is
  Studio's JSON + Arrow IPC client;
- `src/spiral_training_runtime/` is the Python HTTP service;
- `uv.lock` is the reproducible local, CI, and Lambda dependency definition.

The service uses Hugging Face LeRobot's Apache-2.0 ACT implementation directly
through PyTorch. There is no LibTorch or model implementation in the Rust
workspace.

## Parked: training

Training is parked until Spiral decodes MCAP video into training images:
Studio disables starting a run, and `McapTrainingDataset` rejects every job.
The job loop, ACT session, and checkpoint writer stay in place for that day.
Policy inference for rollout works for checkpoints matching the current
schema.

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
`127.0.0.1:50051`. It intentionally stays in the foreground so SSH, `tmux`,
`systemd`, or the cloud-init caller owns process lifetime and logs. Before the
server starts, it discovers the machine's public IPv4 address and prints the
exact tunnel command and Studio endpoint.

On the Studio machine, open the tunnel printed by the bootstrap:

```sh
ssh -N -L 50051:127.0.0.1:50051 ubuntu@LAMBDA_INSTANCE_IP
```

Then connect Studio to:

```text
http://127.0.0.1:50051
```

HTTP remains private to each machine; SSH provides authentication, encryption,
and forwarding without a Lambda firewall rule. Set `SPIRAL_RUNTIME_PORT` to
change both sides of the printed tunnel, `SPIRAL_POLICY_DEVICE` to select the
rollout device, and `SPIRAL_RUNTIME_SSH_USER` if the remote login is not
`ubuntu`. Set `SPIRAL_RUNTIME_PUBLIC_HOST` to override the detected SSH host
with a DNS name or another reachable address. `SPIRAL_RUNTIME_HOST` defaults to
loopback and should remain private.

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

The authoritative training API version and storage-schema compatibility range
live in
[`../crates/backend/training/protocol/version.json`](../crates/backend/training/protocol/version.json).
The current relational schema version lives with
[`storage`](../crates/backend/storage/schema/version.json). Studio
accepts the same API major at its required minor revision or newer, and rejects
unsupported storage schemas before creating a job. See the
[protocol bump policy](../crates/backend/training/protocol/README.md)
for the major/minor rules.

Control plane:

- `GET /v1/health`
- `POST /v1/jobs`
- `GET /v1/jobs`
- `GET /v1/jobs/{id}`
- `POST /v1/jobs/{id}/stop`
- `POST /v1/policies`
- `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`

The immutable job request carries the job ID, the SpiralDB destination, the
selected Spiral schema version, and Studio's run configuration unchanged under
`config`. `POST /v1/jobs` is idempotent only when the job ID and complete
specification match; conflicting reuse returns HTTP 409.

The worker owns each job's history. `GET /v1/jobs` lists every retained job's
state for Studio's run list; `GET /v1/jobs/{id}` adds the job's metrics and log
tail. Metrics are thinned by halving as a run grows, so the retained sample
always spans the whole run. Retained terminal jobs are bounded; Studio shows a
job the worker no longer holds as unavailable, and lists its checkpoints from
Spiral.

Rollout data plane:

- `POST /v1/policies/{id}/infer`

Inference bodies contain one `application/vnd.apache.arrow.stream` record
batch. Tensor columns are non-null `FixedSizeList` arrays. Studio writes Arrow
IPC metadata V5 with lossless Zstandard buffer compression; PyArrow
decompresses those buffers transparently and exposes their child buffers as
zero-copy NumPy views. State children are `Float32`; image children are packed
HWC RGB `UInt8`. The runtime transfers image bytes to the selected device,
then permutes to CHW, converts to `Float32`, divides by 255, and applies
ImageNet normalization.
