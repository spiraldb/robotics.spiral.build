# Spiral training runtime

`training/` owns the complete remote training-worker boundary:

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

## SpiralDB driver

The worker imports the `spiraldb` Python package and opens its ADBC driver with
per-database `username`, `password`, `spiral.api_url`, and `spiral.spfs_url`
options. Until a wheel containing that scoped-option driver is published, set
`SPIRALDB_ADBC_DRIVER` on the worker to the current
`libadbc_driver_spiraldb` shared library. The packaged driver is the fallback.

## Stable protocol

The current training protocol is API version 4. Studio rejects runtimes with a
different API version before creating a job.

Control plane:

- `GET /v1/health`
- `POST /v1/jobs`
- `GET /v1/jobs/{id}`
- `POST /v1/jobs/{id}/stop`
- `GET /v1/jobs/{id}/telemetry`
- `POST /v1/sessions`
- `DELETE /v1/sessions/{id}`
- `GET /v1/sessions/{id}/telemetry`
- `POST /v1/sessions/{id}/checkpoint`
- `POST /v1/policies`
- `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`

The job request is the training control plane and carries the immutable
SpiralDB destination, optional client credentials, SQL/data contract, and
model/run configuration. The runtime opens SpiralDB itself, uses SPQL to load
training-ready Arrow data, and appends SafeTensors directly to
`training_checkpoints`. Job snapshots never return credentials. `POST
/v1/jobs` is idempotent only when the job ID and complete immutable
specification match; conflicting reuse returns HTTP 409. Studio tries `GET`
first when reattaching, so it does not resend credentials after restarting.

Rollout/legacy session data plane:

- `POST /v1/sessions/{id}/buffer?kind=training&step=N`
- `POST /v1/sessions/{id}/train?step=N`
- `POST /v1/sessions/{id}/buffer?kind=validation&step=N`
- `POST /v1/sessions/{id}/validate?step=N`
- `POST /v1/policies/{id}/infer`

Buffer and inference bodies are one `application/vnd.apache.arrow.stream`
record batch. Train and validation execution requests are bodyless and consume
the exact buffered kind and step. Tensor columns are non-null `FixedSizeList`
arrays. Studio writes Arrow IPC metadata V5 with lossless Zstandard buffer
compression; PyArrow decompresses those buffers transparently, then exposes
their child buffers as zero-copy NumPy views. `torch.from_numpy` shares that
decompressed CPU storage.
Training state and action children are `Float32`; image children are packed HWC
RGB `UInt8`; action-padding children are `UInt8`. The runtime transfers the
byte images to the selected device, then permutes to CHW, converts to `Float32`,
divides by 255, and applies ImageNet normalization. A batch therefore sends one
byte per image channel instead of four, while model inputs retain the same
normalized values. Moving a batch to CUDA still necessarily performs one
host-to-device transfer.

The session endpoints remain for compatibility and rollout-focused testing;
Studio's production training path does not call them. Each legacy training
session has one queued-batch slot protected independently from
the model lock. After Studio starts optimizer step N, it uploads batch N+1 into
that slot while PyTorch computes N. Studio never starts N+1 before N completes,
so checkpoints retain exact step semantics. Validation is an explicit barrier
and uses the same slot before the following training batch is admitted.

Legacy session checkpoints are raw SafeTensors bytes with format metadata in
`x-spiral-checkpoint-format` and `x-spiral-checkpoint-extension` headers.
Worker-owned job checkpoints are committed directly to SpiralDB before their
metadata appears in the job snapshot. Studio owns admission and its mirrored
run history; the worker owns the active training data path.
