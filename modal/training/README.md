# Spiral training runtime

`training/` trains two model families on Modal: ACT policies, and progress
models, WARP-RM's progress head for Score progress. `config.model.kind` in the
run request names the family.
[`docs/training.md`](../docs/training.md) describes a run, the Volume, the streams,
and the endpoints.

- `app.py`: the Modal app `spiral-robotics-training`: the CPU control endpoint
  `control` in front of one GPU function per family, `train` (ACT, L4) and
  `train_progress` (H100), sharing the Volume `spiral-robotics-training` and the
  stop-flag Dict `spiral-robotics-training-stops`. Each function builds its
  family's job, in its family's image. `control` also mounts
  `spiral-robotics-models`, the Volume it publishes progress models to and
  [`warp-progress`](../warp_progress/README.md) reads them from.
- Two environments, as the families pin different PyTorch builds:
  `pyproject.toml` and `uv.lock` here are ACT's and the control endpoint's;
  [`../training_progress/`](../training_progress) is the progress job's, Score
  progress's own stack as `warp-progress` locks it (its lock began as that one,
  and `training_progress/tests/test_environment.py` checks it still matches), with no
  LeRobot. Both run the code in `src/`.
- `src/spiral_training_runtime/`:
  - `request.py`, `report.py`: the run request and what a run reports, fixed by
    the fixtures in
    [`crates/backend/src/training/fixtures`](../crates/backend/src/training/fixtures).
  - `control.py`: the control endpoint Studio calls.
  - `publish.py`: publishing a progress checkpoint (`POST /v1/models`): its
    `model.safetensors` and `config.json` copied to
    `spiral-robotics-models` at `models/<sha256>/`, the SHA-256 of the weights,
    renamed into place whole, and left as is when already there.
  - `run.py`: a run's job: prepare, train, report, checkpoint, resume after a
    retry, for either family (`family.py` is what a family gives it).
    `stop.py`: how a stop reaches it. `device.py`: the device and what each
    step on it cost. `validation.py`: the episodes held out.
  - `volume.py`: the Volume's layout and when each side commits and reloads.
  - ACT: `samples.py`, `video.py`, `streams.py`, the prepare step from uploaded
    streams to training samples on the job's local disk; `act.py`, LeRobot's
    ACT and the checkpoints the job writes.
  - Progress: `progress_embeddings.py`, the prepare step, which embeds the
    camera with [`warp-progress`](../warp_progress/README.md)'s own decoding,
    squash and DINOv3 backbone, so training sees pictures exactly as Score
    progress scores them; `progress_windows.py`, WARP-RM's training windows,
    drawn in worker processes; `progress.py`, the head's training (bf16 on CUDA)
    and its checkpoints; `progress_recipe.py`, upstream's sim recipe.
  - `warp_rm_upstream/`: the WARP-RM authors' model, sampler, labeler and loss,
    copied unchanged (MIT) at the commit Score progress was verified against;
    its `__init__.py` says why it is vendored rather than a dependency.
  - `testing.py`: the synthetic streams and fakes both environments' tests use.

## Deploy

The app lives in the `spiral-dev` Modal workspace, beside
[`warp-progress`](../warp_progress/README.md), and accepts the same
bearer token. One-time setup:

```sh
modal token new --profile spiral-dev
modal secret create spiral-robotics-auth MODAL_SERVICE_TOKEN=…
```

The progress image bakes in Score progress's pinned DINOv3 backbone, which is
gated, so its build reads the `spiral-robotics-huggingface` secret that
[`warp-progress`](../warp_progress/README.md#deploy) uses. That build step
imports `app.py`, so the image copies the local sources in before it: a change
to any of them fetches the backbone again on the next deploy.

Then, from `training/`. The workspace defaults to an image builder older than
`uv_sync` supports, so the deploy picks a newer one:

```sh
MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
curl https://spiral-dev--spiral-robotics-training-control.modal.run/v1/health
```

Studio's backend reads that URL as `TRAINING_SERVICE_URL` and the token as
`MODAL_SERVICE_TOKEN`. The ACT and control image installs the locked
dependencies from `uv.lock`, with CUDA PyTorch from the official CUDA 12.8
index declared in `pyproject.toml`; the progress image installs
`../training_progress/uv.lock`. Its path dependencies, `warp-progress` and `video-service`,
are skipped by `uv_sync` and their sources added by path, as the deploy runs in
ACT's environment, where they are not installed.

## Local development

```sh
uv sync --extra dev
uv run pytest
uv run ruff check .
uv run ruff format --check .
(cd ../training_progress && uv sync && uv run pytest)
```

The tests run whole runs of both families on CPU, each in its own environment:
synthetic H.264 and joint streams on a local directory, the control endpoint
with fake function calls, and the jobs themselves. A one-layer DINOv3 stands in
for the gated backbone, two worker processes draw the progress windows, and a
trained progress checkpoint is scored by warp-progress's own `TemporalModel`.
