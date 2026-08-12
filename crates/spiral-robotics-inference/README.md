# Spiral Robotics Native ACT

Native ACT training and inference with no external ML runtime. Studio's server
uses this engine as part of the single Spiral Robotics backend; the binary in
this crate is only a diagnostic interface to the same implementation.

The first compatibility target is Hugging Face LeRobot's established
[`lerobot/act_aloha_sim_transfer_cube_human`](https://huggingface.co/lerobot/act_aloha_sim_transfer_cube_human)
ACT policy:

- revision `ba73b2766f1371cdc133ca4efb97eb090d744625`;
- Apache-2.0 licensed;
- 51.7 million FP32 parameters in SafeTensors;
- ResNet-18 vision backbone, 14-dimensional state and actions, 100-step chunks;
- reported 83% success over 500 ALOHA Transfer Cube simulation episodes.

Model data is streamed from immutable raw HTTPS URLs into Spiral Robotics' local
cache and is never committed to this repository. The downloader does not use a
Hub SDK or API; known files are admitted only after SHA-256 verification.
Set `SPIRAL_ROBOTICS_CACHE_DIR` to override the platform cache directory.

## Run

Download the pinned model:

```sh
cargo run -p spiral-robotics-inference -- download
```

Run a complete forward pass using a black 480×640 frame and zero state:

```sh
cargo run --release -p spiral-robotics-inference -- run
```

Run with a real RGB image and state:

```sh
cargo run --release -p spiral-robotics-inference -- run \
  --image observation.png \
  --state 0,0,0,0,0,0,0,0,0,0,0,0,0,0
```

On Apple platforms, compile Burn's native WGPU backend for Metal:

```sh
cargo run --release -p spiral-robotics-inference \
  --no-default-features --features cli,metal -- run --runs 5
```

The CLI performs one unmeasured warm-up before reported runs. Burn's fused
Metal backend compiles and autotunes the graph during that warm-up; it must be
done while activating a model, never on a robot control-loop deadline. Pass
`--cold` only when intentionally measuring startup behavior.

## Integration boundary

The `Policy` trait accepts:

- a borrowed, flat robot-state slice;
- named borrowed RGB image buffers, including explicit row stride.

It returns an owned row-major action chunk and separate preprocessing, model,
postprocessing, and total latency measurements. Tensor backend types do not
cross this trait.
`Policy::warm_up` gives the runtime an explicit activation step for compiling
backend kernels and rejecting a broken model before motion is possible.
The model specification also exposes the checkpoint-recommended number of
actions to consume from each prediction.
Chunk scheduling, temporal ensembling, deadlines, stale-observation handling,
joint compatibility, and physical safety remain with the integrating robotics
runtime.

Local files and raw HTTPS downloads are artifact-source adapters. Both produce
an owned, content-addressed `ModelArtifact` before ACT parsing begins. An
eventual SpiralDB source can construct that artifact directly from bytes
without materializing paths or changing ACT or the inference interface.

## Trust boundaries

Model loading is deliberately staged so malformed external data cannot become
an ambiguous runtime object:

1. artifact files are copied into process-owned memory and identified by their
   actual SHA-256 digests;
2. remote URLs carry immutable 40-character source revisions, and known
   artifact checksums are verified;
3. JSON is converted into an invariant-preserving `ActConfig`; unsupported
   architectures, malformed features, zero dimensions, and arithmetic
   overflows are rejected here;
4. every checkpoint tensor must match its configuration-derived dtype, name,
   rank, and exact dimensions before becoming a backend tensor;
5. backend output shape and flat storage length are checked independently
   before an `ActionChunk` can be constructed.

The policy graph therefore operates on trusted dimensions rather than
re-validating loosely related JSON and checkpoint values throughout inference.

## Current scope

- batch size one;
- one RGB camera;
- ResNet-18 ACT checkpoints with mean/std normalization;
- stateless full-chunk prediction;
- portable CPU, optional Apple Accelerate, and optional Metal backends.

Portable CPU training and inference are always available. On Apple platforms,
`accelerate` enables an Accelerate-backed CPU path and `metal` enables the
intended local real-time path.

## Verification

The pinned checkpoint is identified by SHA-256
`42772891cb6eba1e7bc36ad8e12c0fa0723c61f036fa235c725ce6026e6e81df`.
For a zero state and black image, all 1,400 action values from this engine were
compared with a separate implementation of the upstream graph; the
maximum absolute difference was `6.26e-7`.
The optional checkpoint test fingerprints all 1,400 outputs at `1e-3`
precision, in addition to checking exact output dimensions and representative
values. Normal CI exercises configuration, artifact identity, tensor-schema,
image-layout, normalization, and action-construction failures without needing
the 207 MB model.

An ignored test can repeat a complete checkpoint inference when the model is
available locally:

```sh
SPIRAL_ACT_MODEL_DIR=/path/to/model \
  cargo test -p spiral-robotics-inference --test act_reference -- --ignored
```

The training-only VAE encoder tensors are present in the upstream checkpoint
but are not loaded for inference. ACT inference uses the zero latent specified
by the reference implementation.
