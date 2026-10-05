# Robot connector API

The robot connector (`crates/robot-connector`) runs next to the arms it drives.
It owns their serial buses, cameras, calibration, and fixed-rate control loop,
records takes, and rolls out policies. Studio's browser app is its only client:
the browser calls it directly at the address saved in **Settings → Robots**,
and Studio's server never reaches it. It never reaches Spiral either. The two
share request and response types through `crates/common`.

```console
cargo run -p spiral-robotics-robot-connector -- --help
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--bind` | `127.0.0.1:7070` | Address the API listens on |
| `--data-dir` | platform data dir`/spiral-robotics/robot-connector` | Saved robots, unsent takes, and policies; also `SPIRAL_ROBOTICS_ROBOT_CONNECTOR_DATA` |
| `--policy-runtime` | `http://127.0.0.1:50051` | Python runtime that computes rollout actions |
| `--no-cameras` | off | Skip camera discovery |
| `--demo` | off | Start with the simulated demo bench below; its data directory defaults to `…/robot-connector-demo` and no physical camera is looked for |
| `--ffmpeg`, `--video-codec` | `ffmpeg`, `libx264` | Camera recording encoder |
| `--max-step`, `--teleop-max-step` | | Largest per-cycle joint change |

Every origin may call it (CORS), since Studio runs at `robotics.spiraldb.com` or
on `localhost`. A page served over HTTPS can reach it at `http://localhost` or
`http://127.0.0.1`; a robot connector on another machine needs HTTPS.

## Conventions

Paths start with `/api`. Failures use Studio's shape and codes,
`{"code": …, "message": …}`, without `unauthenticated` or `stale_project`:
the robot connector has no sessions or projects. As in Studio, a read is
cancelled when its caller disconnects and a command runs to completion
regardless.

The hardware loop and safety stay on the robot connector. Telemetry is a
sampled display snapshot, not the robot control clock. Its `sequence` advances
with hardware publication; repeated HTTP success does not make an unchanged
sample fresh. Recording signals retain their own samples. Read
`/recording/signals` with `generation` and `offset` (initially both zero).
Responses contain at most 1,000 samples. The next offset is the returned offset
plus the number of returned `times`; adopt the returned generation. A
generation change or offset gap means replace the local window.

## Endpoints

- `GET /health`: `{"version": …}`, the build that answers.

Robots, saved in the robot connector's `robots.json`:

- `GET /robots`, `GET /hardware/status`, `GET /robots/telemetry`,
  `GET /serial-ports`.
- `POST /robots/save`: `{document, expected}`. `document` is the complete
  robot JSON object. `expected: null` creates a new robot; otherwise it is the
  revision the editor started from, and a later save fails with 409. Saving
  does not connect hardware.
- `POST /robots/models`: a robot model, `{urdf, meshes}` with each mesh
  base64-encoded under the filename the URDF uses, saved under its content
  address. Answers that address, the SHA-256 of the model's JSON. A robot's
  `visualization.model` names it, and saving a robot whose model is not here
  fails with 404.
- `GET /robots/models/{model}`: the model saved under that address. It never
  changes, so the response may be cached forever; robot configurations stay a
  few kilobytes however large their meshes.
- `DELETE /robots/{id}`; `POST /robots/{id}/{connect,disconnect,hold,release,goal}`;
  `POST /robots/{id}/calibration/{start,finish,cancel}`; `PUT /mode`.
- `POST /cameras/refresh`; `GET /cameras/stream.mjpeg?id=…`, a live MJPEG
  stream for an `<img>`. Camera IDs may contain device-path separators and must
  be percent-encoded.

Takes:

- `POST /recording`: `{dataset_id, leader, follower, video}` starts
  teleoperating the pair and recording a take, with the cameras bound to the
  arms when `video` is true. Answers 201 with the take.
- `PATCH /recording` (`{"paused": …}`), `POST /recording/stop`
  (`{"keep": …}`). Stopping saves the take as an MCAP file; a discarded take is
  saved too, marked as discarded.
- `GET /recording/signals`, `GET /recording/takes/{id}`, and
  `POST /recording/takes/{id}/camera` review the current session's takes.
- `GET /recordings`: the saved takes, each `{dataset_id, episode_id, bytes}`.
- `GET /recordings/{dataset}/{episode}`: one take's MCAP bytes.
- `DELETE /recordings/{dataset}/{episode}`: forget a take Studio has stored.

The browser carries each take to Studio's
`POST /datasets/{dataset}/recordings/{episode}`, then deletes it here, so a
take that fails to reach Studio stays until it does.

Policies and rollout:

- `GET /policies`: the policies written here, each
  `{run_id, checkpoint_step, bytes}`.
- `PUT /policies/{run}/{step}`: a policy envelope as Studio's
  `GET /training/runs/{run}/checkpoints/{step}/policy` serves it.
- `DELETE /policies/{run}/{step}`.
- `POST /rollout` (`RolloutConfig`: `run_id`, `checkpoint_step`, `robot`,
  `duration_seconds`) rolls out a written policy; loading it is what proves it
  is here. `GET /rollout` reads the current session; `POST /rollout/{id}/stop`
  stops it. A restarted robot connector starts idle and never resumes motion.

## Simulated robots

A robot saved with `"simulated": {}` in place of a `port` has motors the robot
connector simulates. They report encoder counts and follow goals at a bounded
speed, so calibration, the control loop, teleoperation, takes, and rollout
treat the arm exactly like a real one. With `"simulated": {"hand_guided": true}`
a simulated hand sweeps the arm through its calibrated travel whenever its
torque is released, the way an operator moves a leader. There is no gravity or
contact.

Every mounted camera on a simulated robot with a model is a simulated camera:
it appears in `GET /hardware/status` under its configured ID at 640×480 and
30 fps, streams from `GET /cameras/stream.mjpeg` like a device, and records
into takes. Its pictures draw every simulated robot with a model, posed from
telemetry, with the renderer the browser's 3D view uses. A mount follows the
optical convention: the camera looks along its +Z, with +X to the right of the
picture and +Y down it.

`--demo` saves two such robots over whatever its data directory held for them:
`sim_leader`, hand-guided, and `sim_follower`, with `wrist` and `front`
cameras. Both are calibrated to TheRobotStudio's SO-101 model, bundled in
`crates/robot-connector/src/simulation/so101`, and connect at start, so a take
can be recorded as soon as Studio reaches the robot connector.
