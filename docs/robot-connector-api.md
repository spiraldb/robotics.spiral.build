# Robot connector API

The robot connector (`robot-connector`) runs next to the arms it drives.
It owns their serial buses, cameras, calibration, and fixed-rate control loop,
and records episodes. It listens on no port: it links itself
to Studio over plain HTTP, presenting the token its workspace issued in
**Rollout → Set up**, so a robot needs no public address or certificate. The
browser reaches it only through Studio. It never reaches Spiral. The two share
request and response types through `crates/common`.

```console
cargo run -p spiral-robotics-connector-os -- --studio https://robotics.spiral.build --token rc_…
```

The flags name the Studio to link to and configure the computer's device
drivers. Everything the connector does is configured from Studio: each robot
carries its own motion limits, and the simulated demo bench is installed from
Rollout → Set up. A microcontroller build has no command line and links the same way.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--studio` | `http://localhost:3000` | Studio's web address; also `SPIRAL_ROBOTICS_STUDIO` |
| `--token` | | The workspace's token; also `SPIRAL_ROBOTICS_CONNECTOR_TOKEN` |
| `--data-dir` | platform data dir`/spiral-robotics/robot-connector` | Saved robots and unsent episodes; also `SPIRAL_ROBOTICS_ROBOT_CONNECTOR_DATA` |
| `--no-cameras` | off | Skip camera discovery |
| `--ffmpeg`, `--video-codec` | `ffmpeg`, `libx264` | Camera recording encoder |

## The link

Every link request carries `Authorization: Bearer <token>`; Studio knows the
workspace from the token. The paths and headers are constants in
`spiral_robotics_common::robot_connector`.

- `GET /api/robot-connector/link/next` is a long poll Studio holds for up to
  25 seconds. It carries `X-Robot-Connector-Health` (the connector's version
  and whether it simulates arms). Every answer carries
  `X-Robot-Connector-Watch`, the live streams someone watches now. A `200`
  hands over one browser request: `X-Robot-Connector-Request`
  (`{id, method, path}`), its content type, and its body. A `204` hands over
  none; Studio answers early when the watched streams change. The connector
  polls again at once, and backs off up to 30 seconds while Studio is
  unreachable.
- `POST /api/robot-connector/link/answers/{id}` returns a request's answer:
  `X-Robot-Connector-Status`, its content type, and its body.
- While watched, `POST /api/robot-connector/link/telemetry` takes the latest
  telemetry about ten times a second, and
  `POST /api/robot-connector/link/cameras?id=…` a camera's latest JPEG about
  five times a second. Episodes record every frame; the preview only frames and
  checks a camera.
- `PUT /api/robot-connector/link/episodes/{dataset}/{episode}?offset=…&size=…`
  uploads a finished episode in parts of at most 4 MiB, each under Cloudflare's
  100 MB request cap. Studio answers `{received}`, the bytes it holds: `200`
  once it kept the part, `409` when the part starts elsewhere, so an upload
  that stopped resumes where Studio's bytes end. Once Studio holds the whole
  episode, the connector deletes its file; an episode Studio refuses (`4xx`) stays
  until the connector restarts. An episode holds at most 256 MiB, one Spiral row.

Studio serves the browser the endpoints below under `/api/robot-connector`
(`/api/robots` is `/api/robot-connector/robots`), answering health, telemetry,
and camera previews from what the connector last sent. It answers
`/episodes`, the whole episodes it holds, itself; `/episodes/{dataset}/{episode}/store`
stores one in its dataset with the browser's Spiral access.

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

- Health is Studio's: `GET /api/robot-connector/health` answers the linked
  connector's `{"version": …, "simulates_arms": …}`, or 503 when none is
  linked. A connector draws simulated arms' cameras with a GPU, so a
  microcontroller does not simulate arms.

Robots, saved in the robot connector's `robots.json`:

- `GET /robots`, `GET /hardware/status`, `GET /serial-ports`. Studio answers
  `GET /robots/telemetry` from the samples the connector posts.
- `POST /robots/save`: `{document, expected}`. `document` is the complete
  robot JSON object. `expected: null` creates a new robot; otherwise it is the
  revision the editor started from, and a later save fails with 409. Saving
  does not connect hardware. A robot's `motion`
  (`{"manual_step_counts", "following_step_counts"}`, 20 and 40 when absent)
  bounds how far the control loop moves it per cycle: gently for a goal set by
  hand, faster while it follows a leader or a policy. A connected robot's
  port, motors, calibration, and motion cannot change; disconnect it first.
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
- `POST /cameras/refresh`. Studio serves `GET /cameras/stream.mjpeg?id=…`, an
  MJPEG preview for an `<img>`, from the frames the connector posts; an
  `<img>` sends no headers, so it names the project in its query too. Camera
  IDs may contain device-path separators and must be percent-encoded.

Episodes:

- `POST /recording`: `{dataset_id, control, video}` starts the control source
  and records an episode of the arms it moves, with their cameras when `video` is
  true. Answers 201 with the episode. `control` is
  `{"teleoperate": {"leader": …, "follower": …}}`: teleoperate the pair and
  record both arms.
- `PATCH /recording` (`{"paused": …}`) pauses or resumes the episode.
  `POST /recording/stop` (`{"keep": …}`) stops the episode and its control source:
  teleoperation returns to idle. The episode's MCAP file is written while it
  records; stopping saves it, and the connector uploads it on the link. A
  discarded episode is saved too, marked as discarded.
- `GET /recording/signals`, `GET /recording/episodes/{id}`, and
  `POST /recording/episodes/{id}/camera` review the paused or last episode, read
  from its file.

A restarted robot connector starts idle and never resumes motion.

## Simulated robots

A robot saved with `"simulated": {}` in place of a `port` has motors the robot
connector simulates. They report encoder counts and follow goals at a bounded
speed, so calibration, the control loop, teleoperation, and episodes treat the
arm exactly like a real one. With `"simulated": {"hand_guided": true}`
a simulated hand sweeps the arm through its calibrated travel whenever its
torque is released, the way an operator moves a leader. There is no gravity or
contact.

Every mounted camera on a simulated robot with a model is a simulated camera:
it appears in `GET /hardware/status` under its configured ID at 640×480 and
30 fps, previews like a device, and records
into episodes. Its pictures draw every simulated robot with a model, posed from
telemetry, with the renderer the browser's 3D view uses. A mount follows the
optical convention: the camera looks along its +Z, with +X to the right of the
picture and +Y down it.

`POST /demo-bench` (Rollout → Set up → Add simulated arms, offered when
`simulates_arms`) saves two such robots over any robots of their names and
connects them: `sim_leader`, hand-guided, and `sim_follower`, with `wrist` and
`front` cameras. Both are calibrated to TheRobotStudio's SO-101 model, bundled
in `robot-connector/src/simulation/so101`, so an episode can be recorded right
away.
