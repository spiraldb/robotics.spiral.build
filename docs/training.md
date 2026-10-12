# Training

Training fits a model to a curated view on a Modal GPU. A run trains one of
two families, its `config.model.kind`:

- **ACT policy** (`act`): drives a robot from its cameras and joints.
- **Progress model** (`progress`): WARP-RM's progress head, trained from
  scratch on one camera's video, so Score progress can score with it. Trained
  on a view of expert demonstrations, it learns what expert pace looks like.

Curation decides **what** to train on: one row per episode, with the time
ranges it keeps of it in `kept`. Training decides **how the model sees it**.
Studio moves the recorded data to Modal compressed, and the job turns it into
training samples next to the GPU.

```
Browser ── POST /api/training/runs ──▶ Studio backend ── 1. upload streams ──▶ Modal Volume
                                            │                                    ▲
                                            ├── 2. POST /v1/runs ─▶ control ─spawn─▶ train (GPU)
                                            └── 3. status, history, checkpoints ◀──┘
```

## A run

1. **Setup.** The operator picks the model family first; switching applies
   that family's defaults. The browser lists the published curated views with
   the dataset each one belongs to, and the dataset's video topics. An ACT run
   takes a view, its cameras, an image size, the training rate, and the ACT
   settings; a progress run takes a view and one camera (the overview camera
   by default). Either starts with that `TrainingConfig`.
2. **Admission** (backend). The config is checked; the view's statement is read
   from Studio's curated views and run once, with each row's `kept` unnested
   into its kept intervals (`episode_id, start_ns, end_ns`, an empty one left
   out; `ProjectQueries::kept_intervals`). Every camera must be a recorded
   video topic of the dataset. For ACT, the dataset's joint roles name the
   state and action streams; a progress run needs no joint roles. The run is
   recorded in Studio SQLite with what it was given, and its upload starts.
3. **Upload** (backend, held in memory). For each episode, each camera's
   packets and, for ACT, each joint stream's values are one **stream**,
   content-addressed by what was read; a progress run's camera stream is the
   same stream an ACT run of that camera uploads. The backend asks Modal which streams it lacks and uploads
   only those, a few at a time, each straight from the Spiral result into the
   request body. Then it asks Modal to start the run. A restarted backend
   loses an upload in progress; the run reads as unavailable and starting it
   again uploads only what is still missing.
4. **Prepare** (Modal). ACT decodes each episode's cameras once, lays the
   training clock over every kept interval, picks each camera's frame and each
   joint stream's value at every tick, resizes the frames, and writes the
   samples to the container's local disk. A progress run decodes its camera
   and embeds every picture of every kept interval with Score progress's own
   code (below), once, onto the local disk.
5. **Train** (Modal). The job samples its batches at random, writes metrics
   as it goes, and checkpoints to the Volume: every checkpoint interval, and
   at each step measuring the lowest validation loss yet. A preempted job is
   retried and resumes from its last interval checkpoint. A run that
   diverges fails at once (see Phases).
6. **Monitor.** The browser polls `/api/training/runs`; the backend answers
   from its records, its uploads, and Modal.
7. **Publish** (progress runs). A progress run's detail lists its
   checkpoints, latest first; naming one and pressing Publish makes it a
   [published model](#published-models) that Score progress can score with.

One run per project is active at a time.

## How the model sees the data

### ACT policy

- **Cameras**: video topics, in the order the policy reads them.
- **Image size**: every camera's frames are resized to one width × height
  with swscale bilinear (PyAV `to_ndarray(width=, height=, format="rgb24")`),
  so a robot running the policy can resize live frames the same way.
- **Rate**: the training clock, in Hz. Each kept interval `[start, end)` gets
  ticks `start + k / rate` for every `k` with the tick before `end`.
- **At each tick**, every camera shows its latest frame presented at or before
  the tick, and every joint stream its latest value at or before the tick
  (sample and hold); a tick before a stream's first sample takes the first.
- **State** concatenates, arm by arm in the joint roles' order, the arm's
  measured joints and then its gripper's measured opening. **Action**
  concatenates the commanded joints and the gripper's commanded opening the
  same way. An arm without commanded positions cannot be trained on.
- **Action chunk**: the action at the tick and at each of the next
  `chunk_size - 1` ticks; ticks past the interval's end are padded
  (`action_is_pad`) with the interval's last action.
- **Normalization**: state and action mean and standard deviation over the
  training ticks, each standard deviation at least `1e-3`; images by
  ImageNet mean and standard deviation.
- **Validation**: whole episodes, `round(validation_split × episodes)` of
  them with halves rounded up (at least one when the split is above zero and
  there are two or more episodes, and never every episode), chosen with the
  run's seed. Both families hold out episodes this way.

### Progress model

It is WARP-RM's sim recipe, the one the released head Score progress uses
(`uynitsuj/warp-rm-sim-bottles-sss15`) was trained with: upstream's
`scripts/train.py --feature-stride 1 --source-standard-stride 15 --crop-mode
squash` at commit `26f9894a`, every other flag at its default. Its constants
live in the job (`modal/training/…/progress_recipe.py`), not in Studio's
config.

- **Pictures**: every frame of the camera, which must be 30 Hz video (Score
  progress scores only that). Each is decoded, converted, squashed to
  224 × 224 and embedded by DINOv3 ViT-B/16 (frozen, its final-norm CLS
  token, 768 values) with `warp_progress`'s own code, so training sees
  pictures exactly as scoring does. An interval with no picture is skipped;
  the log counts those, and the intervals shorter than one standard-pace
  window (467 pictures), which still train.
- **Windows**: 32 embeddings from one interval, laid by upstream's AR(1)
  time-warp sampler (path 0.5–2.5 s per step, reversals, a full flip half the
  time), labelled with their cumulative progress relative to the window's
  first picture at a standard stride of 15 pictures.
- **Head**: upstream's `TransformerAggregator` (12 layers, 8 heads, temporal
  differences, stochastic depth 0.1, dropout 0.15) with the relative C51
  head, 30 bins over ±3, and no absolute-progress head (`no_abs`).
- **Optimizer**: AdamW at the run's rate (4e-4 by default), weight decay
  1e-3, gradients clipped at 1.0, a 1,000-step warmup then upstream's cosine
  schedule. The forward pass and loss run in bf16 on CUDA (upstream trains in
  fp32); weights, optimizer state and checkpoints stay fp32. Batches are
  windows (1,024 by default) drawn from intervals picked uniformly, built in
  parallel by six worker processes over the embeddings, memory-mapped.
- **Validation loss**: the training loss on windows of the held-out episodes.
  The step measuring the lowest one yet is checkpointed, so the best model
  is kept even when it comes early: on the 50-episode sample it came near
  step 750 of 15,000, every run overfitting after.

Upstream's model, sampler, labeler and loss are copied verbatim, with their
MIT license, into `modal/training/…/warp_rm_upstream/`; the package itself
cannot be installed as published (it imports files it does not ship). The
loop is ours, so stop, resume and history work as for ACT. The progress job
runs in its own environment, `modal/training/progress/`, which pins Score
progress's packages at the same versions (a test compares the two locks), so
pictures are embedded by the same torch and transformers that score them;
ACT's environment, with LeRobot, is separate.

## The Modal service

One Modal app, `spiral-robotics-training`, in
[`modal/training/`](../modal/training), with the Volume
`spiral-robotics-training` and a CPU control endpoint in front of one GPU
function per family, in the `spiral-dev` Modal workspace: `train`, ACT on an
L4, and `train_progress`, the progress model on an H100, whose image holds
the DINOv3 backbone. Studio reaches it with
`TRAINING_SERVICE_URL` and the bearer token `MODAL_SERVICE_TOKEN`, which
warp-progress accepts too: deployment secrets read from the backend's
environment like the other provider keys.

### Volume layout

```
streams/<key[0..2]>/<key>.arrow                      one uploaded stream
runs/<project>/<run_id>/request.json                 the run request, as posted
runs/<project>/<run_id>/call.json                    {"function_call_id": "…"}
runs/<project>/<run_id>/status.json                  written by the job, or by the control endpoint for a run stopped before its job started
runs/<project>/<run_id>/history.json                 metrics and log tail
runs/<project>/<run_id>/checkpoints/<step>/config.json
runs/<project>/<run_id>/checkpoints/<step>/model.safetensors
runs/<project>/<run_id>/resume.pt                    latest model + optimizer state
```

### Streams

Every stream is an Arrow IPC stream with one or more record batches, its rows
in recorded order.

- **Camera**: `log_time_ns` int64 and `payload` binary, both non-null; one row
  per recorded H.264 Annex-B access unit, in decode order. Schema metadata
  `codec=h264`, `framing=annex_b`.
- **Values**: `log_time_ns` int64, then `v0`, `v1`, … float64 (nullable); one
  row per recorded message, in log-time order. A row with a null value is
  skipped.

`log_time_ns` is the recording's log time, the clock the view's `kept`
ranges use: absolute for MCAP, from the episode's start for a LeRobot
episode, whose cameras are its video features and whose joints are the named
members of `observation.state` and `action` (`RecordedValues::members`). A key is the lowercase hex SHA-256 of the JSON array
`[dataset resource id, dataset version id, episode id, statement]`: the same
read of the same dataset version is uploaded once, by any run.

### Endpoints

All but `/v1/health` need `Authorization: Bearer <token>`. Errors answer
`{"error": "…"}`.

| Method | Path | Body | Answer |
| --- | --- | --- | --- |
| GET | `/v1/health` | | `{"api_version": 1, "gpu": "L4"}` |
| POST | `/v1/streams/missing` | `{"keys": […]}` | `{"missing": […]}` |
| PUT | `/v1/streams/{key}` | Arrow IPC stream | 204 |
| POST | `/v1/runs` | run request | run status, 201; the same request again answers its status, a different one under the same ID 409 |
| GET | `/v1/runs/{project}?ids=a,b` | | `[run status]`, only the runs it holds |
| GET | `/v1/runs/{project}/{run_id}` | | run history; 404 when unknown |
| POST | `/v1/runs/{project}/{run_id}/stop` | | run status |
| DELETE | `/v1/runs/{project}/{run_id}` | | 204; 409 while active |
| POST | `/v1/models` | `{project, run_id, step}` | `{model, manifest}`, 201 when first published, 200 when those weights already were; 404 for an unknown run or step, 409 for a checkpoint that is not a progress model |

The run request, status, and history are fixed by the fixtures in
[`crates/backend/src/training/fixtures`](../crates/backend/src/training/fixtures),
which the Rust and Python tests both read. The request carries Studio's
`TrainingConfig` unchanged under `config`, its `model` either
`{"kind": "act", observation, chunk_size, architecture}` or
`{"kind": "progress", camera}`. Each episode lists its kept intervals as
`[start_ns, end_ns]` and its streams. An ACT request
(`run-request.json`) names each state and action value in `joints`, and each
episode has one camera stream per `config.model.observation.cameras` entry, in
that order, and the state and action value streams whose columns
concatenate, in order, into the state and the action. A progress request
(`progress-run-request.json`) has no `robot_type` or `joints`, and each
episode has its one `camera` stream.

**Phases** a run reports: `starting` (spawned, not yet running), `preparing`,
`running`, `stopping`, `completed`, `stopped`, `failed`. Studio adds
`uploading` while it uploads and `unavailable` when nobody can say. A stop
sets a flag the job reads between steps; it saves a checkpoint and ends
`stopped`. A run stopped before its job starts ends `stopped` without one. A
run whose function call ended while its status still says active reads
`failed`. A run that diverges ends `failed` at once, saying where and which
checkpoint has the lowest validation loss; it keeps its checkpoints and saves
none of the diverged weights. Diverged means the training loss, averaged over
100 steps, held above three times its lowest such average for 250 steps
(`divergence.py`): a collapsed model, which never recovers.

Checkpoints are saved every checkpoint interval, and at each step measuring
the lowest validation loss yet; that best one replaces the best one before it
unless the interval also asked for it, so a run keeps at most one checkpoint
beyond its interval ones. Only interval checkpoints carry the state a retry
resumes from.

### Checkpoints

Studio lists every run's checkpoints with their family; only an ACT
checkpoint has a policy, and only it is offered to Rollout, which cannot run
it on a robot yet.

An ACT checkpoint is `config.json`, the LeRobot ACT policy config plus
`spiral_camera_order` and `spiral_sample_hz`, and `model.safetensors`, the
policy's tensors with the normalization buffers and, in its metadata, the
manifest `spiral_robotics.act.safetensors.v2` (`data_contract` with
`robot_type`, `joint_names`, `camera_streams`, `sample_hz`, `image_size`).
Camera feature keys are `observation.images.` plus the topic with `/` and `-`
made `_` (`/top-camera` → `observation.images.top_camera`).

A progress checkpoint is `model.safetensors`, the head's tensors under
upstream `TransformerAggregator` names (what Score progress's `TemporalModel`
loads, as it loads the released head), and `config.json`, the manifest
`spiral_robotics.progress.safetensors.v1`: `family`, `run_id`, `camera`,
`backbone` (`facebook/dinov3-vitb16-pretrain-lvd1689m@<revision>`),
`window_size`, `feature_stride`, `standard_stride_src`, `fps`, `bins`,
`bin_range`, `ablation`, `image`, `crop` and `step`. Its `resume.pt`, about
1 GB, also holds the cosine schedule's state.

Stored runs from before the families were moved into this shape by the Studio
migration `m20261008_000002_training_models`.

## Published models

A progress checkpoint becomes a model Score progress can score with when it is
published under a name, such as "Bottle experts".

- **Weights.** The control endpoint (`POST /v1/models`) copies the
  checkpoint's `model.safetensors` and `config.json` into the Volume
  `spiral-robotics-models`, at `models/<sha256>/`, keyed by the SHA-256 of the
  weights. It copies into a `.partial` directory and renames it whole, so a
  reader never sees half a model, and leaves a model already there as it is:
  the same weights are one model, however often they are published. Only the
  control endpoint writes this Volume, and deleting a training run leaves its
  published models intact.
- **Identity.** A published model is the string
  `spiral-robotics-models/<sha256>`, the `model` Score progress's requests
  carry and its stored answers' column is keyed by.
- **Record.** Studio keeps each publication in the SQLite table
  `published_models` (`project`, `name`, `model`, `camera`, `run_id`, `step`,
  `created_ms`): names are unique in a project in any case, and the same
  weights may be published under several names. Project bundles carry it.
  `POST /api/training/models` (`{run_id, step, name}`) checks the run is a
  progress run with that checkpoint, publishes, and records it;
  `GET /api/progress-models` lists the project's models.
- **Scoring.** `warp_progress` mounts the models Volume read-only. Asked for a
  published model it does not hold, it reloads the Volume, checks the
  manifest matches what it runs (backbone revision, window, strides, rate,
  bins, ablation, crop, image size), and keeps the head in memory. An unknown
  model answers 404 and an incompatible one 409, and either ends Studio's
  fill without storing an answer.
