# warp-progress

A Modal GPU service that scores WARP-RM task progress for one episode's top-camera video. It is a
PyTorch port of SpiralDB's `spql.ai.warp_rm`
(`engine/plugins/spiral-ai/src/warp.rs`, `warp/dinov3.rs`, `warp/temporal.rs`) with the same pinned
checkpoint: `uynitsuj/warp-rm-sim-bottles-sss15@6417be05…` over DINOv3 ViT-B/16 `@5931719e…`.

- `warp_progress/video.py`: H.264 Annex-B packets decoded to display-order RGB with PyAV. The colour
  conversion is the one spiral-video uses: swscale at the decoded size with `FAST_BILINEAR`.
- `warp_progress/warp.py`: the squash resize (integer maths on the model's device), the backbone (transformers' `DINOv3ViTModel`), the temporal
  head, the window plan and the dense scores. On CUDA both models run under fp16 autocast, about 3x
  faster on a T4; velocity moves by at most about 0.02 against fp32. On MPS and CPU they stay in fp32,
  the closest match to the Rust.
- `warp_progress/service.py`: the HTTP contract, with `POST /score_progress` (bearer token, Arrow IPC in
  and out) and `GET /health`. `/health` reports the service `revision`
  (`warp-progress-0.2.0-fp16`). Every request must carry that revision as schema metadata
  `implementation`; any other value gets a 409, so a client never stores answers from another
  revision as rejected videos (422). Each response carries a `Server-Timing` header with the request's
  stages (`request`, `decode`, `copy`, `queue`, `squash`, `backbone`, `temporal`, `response`), and the
  same line is logged.
- `warp_progress/timing.py`: the per-stage timer behind that header.
- `app.py`: the Modal app `warp-progress`. It runs on T4s and bakes the weights into the image. Each
  container accepts several requests at once: they upload and decode concurrently, while the model
  serves one batch at a time on the GPU (`queue` is the wait for it).
- `parity.py`: compares the port with the Rust function on the hosted sample dataset.

## Local work

```sh
uv sync
uv run pytest
uv run python parity.py            # MPS when available; --device cpu for CPU
```

`parity.py` reads the pyspiral login, `HF_TOKEN` and Studio's release ADBC driver. It expects the
weights in `~/.cache/warp-progress/artifacts`. To fetch them, run
`HF_TOKEN=… uv run python -c 'from pathlib import Path; from warp_progress.artifacts import Artifacts; Artifacts(Path.home() / ".cache/warp-progress/artifacts").download()'`.

## Deploy

One-time setup. DINOv3 is gated, so the Hugging Face token needs access to it:

```sh
modal token new
modal secret create huggingface HF_TOKEN=…
modal secret create warp-progress-auth WARP_PROGRESS_TOKEN=…
```

Then:

```sh
uv run modal deploy app.py
curl https://<workspace>--warp-progress-warpprogress-web.modal.run/health
```

The image build downloads both artifacts with the `huggingface` secret. Running containers need no
token. The build copies `warp_progress/` in before that download, so any code change downloads the
weights again on the next deploy.
