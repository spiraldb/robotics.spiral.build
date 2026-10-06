# Spiral training runtime

`training/` trains ACT policies on Modal and serves their checkpoints for rollout.
[`docs/training.md`](../docs/training.md) describes a run, the Volume, the streams,
and the endpoints.

- `app.py`: the Modal app `spiral-robotics-training`: the CPU control endpoint
  `control` in front of the L4 function `train`, sharing the Volume
  `spiral-robotics-training` and the stop-flag Dict
  `spiral-robotics-training-stops`.
- `src/spiral_training_runtime/`:
  - `request.py`, `report.py`: the run request and what a run reports, fixed by
    the fixtures in
    [`crates/backend/src/training/fixtures`](../crates/backend/src/training/fixtures).
  - `control.py`: the control endpoint Studio calls.
  - `run.py`: a run's job: prepare, train, report, checkpoint, resume after a
    retry. `stop.py`: how a stop reaches it.
  - `samples.py`, `video.py`, `streams.py`: the prepare step, from uploaded
    streams to training samples on the job's local disk.
  - `volume.py`: the Volume's layout and when each side commits and reloads.
  - `act.py`: LeRobot's ACT, shared by training and inference, so a checkpoint
    the job writes loads for rollout.
  - `service.py`: the local policy runtime the robot connector calls.

## Deploy

The app lives in the `spiral-dev` Modal workspace, beside
[`warp-progress`](../modal/warp_progress/README.md), and accepts the same
bearer token. One-time setup:

```sh
modal token new --profile spiral-dev
modal secret create spiral-robotics-auth MODAL_SERVICE_TOKEN=…
```

Then, from `training/`. The workspace defaults to an image builder older than
`uv_sync` supports, so the deploy picks a newer one:

```sh
MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
curl https://spiral-dev--spiral-robotics-training-control.modal.run/v1/health
```

Studio's backend reads that URL as `TRAINING_SERVICE_URL` and the token as
`MODAL_SERVICE_TOKEN`. The image installs the locked dependencies from
`uv.lock`; the job reads its CUDA PyTorch from the official CUDA 12.8 index
declared in `pyproject.toml`.

## Local development

```sh
uv sync --extra dev
uv run pytest
uv run ruff check .
uv run ruff format --check .
```

The tests run a whole run on CPU: synthetic H.264 and joint streams on a local
directory, the control endpoint with fake function calls, and the job itself.

## Policy runtime for rollout

```sh
uv run spiral-training-runtime --host 127.0.0.1 --port 50051
```

The robot connector loads a checkpoint with `POST /v1/policies`, then calls
`POST /v1/policies/{id}/warm-up` and `POST /v1/policies/{id}/infer`.
Inference bodies contain one `application/vnd.apache.arrow.stream` record
batch. Tensor columns are non-null `FixedSizeList` arrays: `Float32` state and
packed HWC RGB `UInt8` images, which PyArrow exposes as zero-copy NumPy views.
The runtime moves image bytes to the device, then permutes to CHW, converts to
`Float32`, divides by 255, and applies ImageNet normalization. It refuses
non-loopback listeners unless `--allow-remote-access` is passed;
`SPIRAL_POLICY_DEVICE` selects its device.
