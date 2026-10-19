# warp-progress

A Modal GPU service that scores WARP-RM task progress for one episode's top-camera video. It is a
PyTorch port of SpiralDB's `spql.ai.warp_rm`
(`engine/plugins/spiral-ai/src/warp.rs`, `warp/dinov3.rs`, `warp/temporal.rs`) with the same pinned
checkpoint: `uynitsuj/warp-rm-sim-bottles-sss15@6417be05…` over DINOv3 ViT-B/16 `@5931719e…`. It also
scores with the progress models Studio's [training](../training/README.md) publishes, each a head
trained for this same backbone and recipe.

- `warp_progress/warp.py`: the pictures in display order, converted to RGB as spiral-video converts
  them (swscale at the decoded size with `FAST_BILINEAR`), the squash resize (integer maths on the model's device), the backbone (transformers' `DINOv3ViTModel`), the temporal
  head, the window plan and the dense scores. On CUDA both models run under fp16 autocast, about 3x
  faster on a T4; velocity moves by at most about 0.02 against fp32. On MPS and CPU they stay in fp32,
  the closest match to the Rust. `PictureEncoder`, the pictures through the backbone, is also what the
  [training job](../training/README.md) embeds a progress model's pictures with, so it trains on them
  exactly as they are scored; it takes this package as a path dependency.
- `warp_progress/published.py`: the published progress models, on the Modal Volume
  `spiral-robotics-models` at `models/<sha256>/{model.safetensors,config.json}`, the hash being that
  of the weights. A model is checked against what this scorer runs (backbone, window, strides, rate,
  bins, ablation, crop, image size) before its head loads as the released one does.
- `warp_progress/service.py`: `POST /score_progress` on the [shared video service
  contract](../video_service/README.md), revision `warp-progress-0.2.0-fp16`. A request's `model` is
  the released selector or `spiral-robotics-models/<sha256>`; any other, or a hash nobody published,
  is a 404, and a published model made for another scorer a 409. Its stages are `request`, `decode`,
  `copy`, `queue`, `squash`, `backbone`, `temporal` and `response`.
- `app.py`: the Modal app `warp-progress`. It runs on T4s, bakes the released weights into the image,
  and mounts `spiral-robotics-models` read-only. A container loads a published head the first time a
  request names it, after reloading the Volume, so a model published since it started is found, and
  keeps it. Each container accepts several requests at once: they upload and decode concurrently,
  while the model serves one batch at a time on the GPU (`queue` is the wait for it).
- `warp_progress/testing.py`: stand-ins for the gated backbone and for a head, and a local directory
  standing in for the models Volume, for these tests and the training job's.

## Local work

```sh
uv sync
uv run pytest
```

## Deploy

The app lives in the `spiral-dev` Modal workspace, beside the
[training service](../training/README.md), and accepts the same bearer token.
One-time setup. DINOv3 is gated, so the Hugging Face token needs access to it:

```sh
modal token new --profile spiral-dev
modal secret create spiral-robotics-huggingface HF_TOKEN=…
modal secret create spiral-robotics-auth MODAL_SERVICE_TOKEN=…
```

Then, with the image builder `uv_sync` needs (the workspace defaults to an older one):

```sh
MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
curl https://spiral-dev--warp-progress-warpprogress-web.modal.run/health
```

The image build downloads both artifacts with the `spiral-robotics-huggingface` secret. Running containers need no
token. The build copies `warp_progress/` in before that download, so any code change downloads the
weights again on the next deploy.
