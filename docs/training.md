# Training

Training fits an ACT policy to a curated view on a Modal GPU. Curation decides
**what** to train on: one row per kept interval of an episode. Training
decides **how the model sees it**: which cameras, at what image size, on what
clock. Studio moves the recorded data to Modal compressed, and the job turns it
into training samples next to the GPU.

```
Browser ── POST /api/training/runs ──▶ Studio backend ── 1. upload streams ──▶ Modal Volume
                                            │                                    ▲
                                            ├── 2. POST /v1/runs ─▶ control ─spawn─▶ train (GPU)
                                            └── 3. status, history, checkpoints ◀──┘
```

## A run

1. **Setup.** The browser lists the published curated views with the dataset
   each one belongs to, and the dataset's video topics. The operator picks a
   view, its cameras, an image size, the training rate, and the ACT settings,
   and starts a run with that `TrainingConfig`.
2. **Admission** (backend). The config is checked; the view's statement is read
   from Studio's curated views and run once to list its kept intervals
   (`episode_id, start_ns, end_ns`); the dataset's joint roles name the state
   and action streams. The run is recorded in Studio SQLite with what it was
   given, and its upload starts.
3. **Upload** (backend, held in memory). For each episode, each camera's
   packets and each joint stream's values are one **stream**, content-addressed
   by what was read. The backend asks Modal which streams it lacks and uploads
   only those, a few at a time, each straight from the Spiral result into the
   request body. Then it asks Modal to start the run. A restarted backend
   loses an upload in progress; the run reads as unavailable and starting it
   again uploads only what is still missing.
4. **Prepare** (Modal). The job decodes each episode's cameras once, lays the
   training clock over every kept interval, picks each camera's frame and each
   joint stream's value at every tick, resizes the frames, and writes the
   samples to the container's local disk.
5. **Train** (Modal). ACT samples ticks at random, writes metrics as it goes,
   and checkpoints to the Volume. A preempted job is retried and resumes from
   its last checkpoint.
6. **Monitor.** The browser polls `/api/training/runs`; the backend answers
   from its records, its uploads, and Modal.

One run per project is active at a time.

## How the model sees the data

- **Cameras**: video topics, in the order the policy reads them.
- **Image size**: every camera's frames are resized to one width × height
  with swscale bilinear (PyAV `to_ndarray(width=, height=, format="rgb24")`),
  so a rollout can resize live frames the same way.
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
  run's seed.

## The Modal service

One Modal app, `spiral-robotics-training`, in [`training/`](../training), with
the Volume `spiral-robotics-training` and a CPU control endpoint in front of
the GPU function, in the `spiral-dev` Modal workspace. Studio reaches it with
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

`log_time_ns` is the absolute MCAP log time, the clock the view's
`start_ns`/`end_ns` use. A key is the lowercase hex SHA-256 of the JSON array
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
| GET | `/v1/runs/{project}/{run_id}/checkpoints/{step}/config` | | the policy config JSON |
| GET | `/v1/runs/{project}/{run_id}/checkpoints/{step}/model` | | the SafeTensors weights |

The run request, status, and history are fixed by the fixtures in
[`crates/backend/src/training/fixtures`](../crates/backend/src/training/fixtures),
which the Rust and Python tests both read. The request carries Studio's
`TrainingConfig` unchanged under `config`; `joints` names each state and
action value; each episode lists its kept intervals as `[start_ns, end_ns]`
and its streams: one camera stream per `config.observation.cameras` entry, in
that order, and the state and action value streams whose columns concatenate,
in order, into the state and the action.

**Phases** a run reports: `starting` (spawned, not yet running), `preparing`,
`running`, `stopping`, `completed`, `stopped`, `failed`. Studio adds
`uploading` while it uploads and `unavailable` when nobody can say. A stop
sets a flag the job reads between steps; it saves a checkpoint and ends
`stopped`. A run stopped before its job starts ends `stopped` without one. A
run whose function call ended while its status still says active reads
`failed`.

### Checkpoints

A checkpoint is `config.json`, the LeRobot ACT policy config plus
`spiral_camera_order` and `spiral_sample_hz`, and `model.safetensors`, the
policy's tensors with the normalization buffers and, in its metadata, the
manifest `spiral_robotics.act.safetensors.v2` (`data_contract` with
`robot_type`, `joint_names`, `camera_streams`, `sample_hz`, `image_size`).
Camera feature keys are `observation.images.` plus the topic with `/` and `-`
made `_` (`/top-camera` → `observation.images.top_camera`).
