# object-tracking

A Modal GPU service that tracks what Studio's Track objects node names, such as "bottle", through one
episode's camera video with SAM 3.1, pinned at `facebook/sam3.1@daa63191…` and the SAM 3 code at the
3.1 release (`5a3143c`; later commits break SAM 3.1 video sessions).

- `object_tracking/video.py`: the episode's pictures, keeping the first and then one every 200 ms of
  presentation time — five a second — in display order. Episodes of more than 3,000 sampled pictures
  (ten minutes) are refused.
- `object_tracking/tracker.py`: `TrackerPool`, two worker processes per H100, each with its own SAM
  3.1 multiplex predictor at its released 1008 pixels, sharing the GPU through NVIDIA MPS. One
  process leaves the GPU a third idle, launching SAM 3.1's many small kernels from Python; two under
  MPS fill it, and three overflow 80 GB on long episodes. Frames wait in host memory
  (`offload_video_to_cpu`), so episode length never decides what fits on the GPU.
- `object_tracking/service.py`: `POST /track_objects` on the [shared video service
  contract](../video_service/README.md), revision `object-tracking-0.1.0`. A request also names what
  to track as schema metadata `prompt`. A tracker failure, such as running out of GPU memory, is a 503
  to ask again. The answer is one row per sampled picture: `timestamp_ns` uint64 and `objects`, each
  with its track `id`, `score`, and box `x`, `y`, `width`, `height` as fractions of the picture from
  its top left. Its stages are `request`, `decode`, `track` and `response`.
- `app.py`: the Modal app `spiral-robotics-object-tracking`. It runs on H100s and bakes the checkpoint
  into the image; each container takes six requests at once, two tracking while the rest upload and
  decode.

Measured on 100 episodes of the sim-bottles sample against their recorded bottle counts (2 to 6), the
most objects one sampled picture held matched the count in 88 of them and was off by one in the rest,
at 19.6 pictures a second per H100. Running SAM 3.1 at 840 pixels instead counted as well, at 23.2
pictures a second with three workers, but needs SAM 3's builder changed in about ten places.

## Local work

```sh
uv sync
uv run pytest
```

The tests run the contract and the sampling against a stand-in tracker; no GPU or weights are needed.

## Deploy

The app lives in the `spiral-dev` Modal workspace, beside [`warp-progress`](../warp_progress/README.md)
and the [training service](../training/README.md), and accepts the same bearer token. SAM 3.1 is gated,
so the Hugging Face token in `spiral-robotics-huggingface` needs access to `facebook/sam3.1`.

```sh
MODAL_IMAGE_BUILDER_VERSION=2025.06 uv run modal deploy app.py
curl https://spiral-dev--spiral-robotics-object-tracking-objecttracking-web.modal.run/health
```

Studio's backend reads that URL as `OBJECT_TRACKING_URL` and the token as `MODAL_SERVICE_TOKEN`.
