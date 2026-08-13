# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

The primary user is an **ML engineer building robot policies**. Their real job
is collecting data, training models, and evaluating behavior; Spiral Robotics
is the hardware and data layer they need to stop fighting. They judge it on
how quickly and reliably they can get from “arms on the bench” to synchronized,
queryable episodes, and on whether the system stays predictable once physical
motion begins.

They work with a pair of SO-ARM101 arms — one leader and one follower — wired
to the Spiral Robotics backend, with one or more cameras observing the workspace. The
browser may be on the same machine or elsewhere on the network.

Secondary, and not yet a design target: other SO-101 owners who will arrive
when the project is opened. See **Distribution** below.

## Product Purpose

Spiral Robotics sets up, calibrates, and drives SO-ARM101 arms, records synchronized
robot and camera data, publishes and queries episodes, trains policies, and
safely replays actions on a follower. Operators use it from the browser or the
terminal without managing a separate ML runtime themselves.

The durable scope is the **full robot-learning loop**: connect → calibrate →
teleoperate → record → inspect/query → publish/export → train → run a policy.
The loop now crosses the Studio backend boundary: vision-conditioned ACT
training publishes a native artifact, and Rollout loads, supervises, measures,
and applies it through the follower's bounded control path.

Success is an ML engineer collecting a usable dataset, understanding any
episode against one synchronized clock, training from Spiral, and replaying it
on real hardware without writing glue code or operating a second training stack.

## Positioning

- **One trusted Rust backend, one portable browser frontend.** The backend owns
  hardware, safety, recording, media encoding, data services, and replay. The
  browser is an interchangeable operating surface that talks only to that
  backend; it never holds service credentials or drives hardware directly.
- **Rust owns the system boundary.** The motor bus, camera capture,
  calibration, kinematics, server, run supervision, data contracts, and web
  interface are Rust. Policy training is also native: a dedicated ADBC
  connection streams Arrow state, action, and Spiral-decoded image tensors into
  Burn's full vision ACT/CVAE trainer. Rust owns lifecycle, cancellation,
  validation, telemetry, and SafeTensors checkpoint publication.
- **One arm is one robot instance is one JSON file.** Serial port, cameras,
  motors, and calibration live in one versioned file. There is no hidden
  calibration cache or second manifest to reconcile, and the Studio notices
  when another tool changes the file.
- **Episode data keeps its natural streams and one clock.** Joint state,
  commanded action, camera media, and recording gaps are separate synchronized
  streams rather than one wide table of duplicated or resampled values. The
  Timeline is the common inspection and control surface for that clock.
- **SpiralDB is the primary connected data path.** Completed episodes can be
  published as relational Arrow tables plus managed MP4 data through ADBC,
  queried from the Studio, and brought back through the backend for safe local
  replay. Local JSONL/MP4 is the operator-cleared staging and recovery buffer,
  not a second connected dataset.
- **The robot description is the single source of truth.** Geometry, joint
  limits, and the rendered model come from URDF rather than duplicated drawing
  constants.

## Operating Context

The deployment boundary is deliberately simple:

```text
Browser frontend
      │ HTTP + event stream
      ▼
Spiral Robotics backend
      ├── robot connections and safety
      ├── recording and MP4 encoding
      ├── local episode recovery
      ├── SpiralDB ingest and read-only queries
      ├── local policy-training supervision and telemetry
      └── episode validation and follower replay
```

- **The browser can run anywhere.** It renders the interface, WebGL scene,
  charts, tables, and native video. It connects only to the Spiral Robotics
  backend and does not connect directly to robots or SpiralDB.
- **The backend is the one consequential system.** It normally runs near the
  arms and owns all hardware connections, the control loop, safety decisions,
  camera capture, video encoding, durable episode files, service credentials,
  database work, and replay execution.
- **The wire API is a product boundary.** The Dioxus browser is the current
  client, not a privileged bypass. Hardware and safety behavior must remain
  correct for any client that uses the API.
- **Network degradation is normal operating context.** Reconnection, stale
  telemetry, and unavailable services must suspend or block consequential
  actions without pretending the physical system is safe merely because the
  browser lost contact.
- **Replay never depends on a live database stream.** The backend obtains and
  validates the action trajectory before motion, then executes through the
  same bounded local control path used by local episodes.
- **Service settings are backend-scoped.** Secrets are stored and used by the
  backend. The browser receives configuration state, not stored tokens.

The physical scene matters: an operator may have one hand supporting an arm
during calibration or teleoperation. Releasing torque lets an arm fall under
its own weight, so controls and consequences must remain reachable and stable.

Studio policy execution is backend-owned. Training exports the exact native ACT
graph and normalization contract as SafeTensors; rollout loads that artifact
from Spiral, warms the Burn backend, binds its ordered state and camera inputs,
and admits one session-scoped policy mode to the bounded motor control thread.
The backend stops the session on stale inputs, motor faults, mode changes,
inference failures, or excessive observation-to-action latency. The browser
selects and monitors a rollout but never becomes the motor-control runtime.

## Capabilities and Constraints

**Shipped today**

- Three CLIs — `spiral-robotics-find-port`, `spiral-robotics-find-cameras`, and
  `spiral-robotics-calibrate` — cover the Studio’s hardware setup flow for scripting and
  machines without a browser.
- A fullstack Dioxus Studio with four primary operating activities:
  - **Live** owns recording, arm selection, Hold / Release / Teleop, joint
    control, health, and camera status.
  - **Data** owns the episode catalog, automatic SpiralDB publication and retry
    visibility, replay/delete actions, import and export, and guarded read-only
    Spiral SQL.
  - **Training** owns random anchor and camera/action sampling through the
    Spiral ADBC/Arrow path, local Burn vision ACT configuration, learning and
    throughput telemetry, graceful cancellation, and Spiral checkpoints.
  - **Rollout** owns checkpoint selection, fixed-follower readiness, simple
    action-chunk and control-rate settings, the live 3D scene, every follower
    camera recorded for the policy, authoritative start/stop lifecycle, and
    measured native inference evidence.
  - **Settings** owns robot setup (ports, camera assignment, calibration,
    add/import/delete) and backend data destinations. Setup is not mixed into
    the live operating flow.
- A reusable `spiral-robotics-panels` crate supplies tabbed tiles, horizontal and
  vertical splits, drag docking with five-zone previews, split resizing,
  keyboard equivalents, persistence, panel reconciliation, and compact shared
  chrome. Live and Data keep independent saved layouts. The default desktop
  composition is a one-third workbench beside a two-thirds visualization, with
  Scene/Query, camera panels, and Timeline as movable panels rather than fixed
  slots.
- Four backend motion modes: **Idle**, **Manual**, **Teleoperation**, and
  **Replay**. The Studio presents their operator-facing form as **Hold**
  (torque engaged and joints writable), **Release** (torque off and telemetry
  read-only), and **Teleop** (leader mapped to follower).
- A live 3D scene drawn from URDF, live MJPEG camera panels, and one stable
  stream connection per assigned camera.
- Episode recording during Teleop with server-owned start, pause, resume, stop,
  and discard. Pause/resume preserves one episode identity and records explicit
  time gaps. Assigned cameras can be encoded by the backend into browser-
  playable MP4 segments with episode-clock offsets. A completed recording is
  queued automatically for the configured SpiralDB destination.
- One canonical **Timeline** in both Live and Data. During capture it follows
  new joint and camera data. A paused or completed episode can be scrubbed to
  synchronize recorded camera frames, historical 3D pose, signals, and the
  playhead. Physical time-seeking is separate, explicit, server-gated, and
  applied on committed release rather than every pointer move.
- Dataset semantics are explicit: one dataset contains many episodes. An
  episode can be selected, deleted, or replayed on a compatible follower.
  LeRobot v2/v3 archives can be imported; export currently produces LeRobot v3
  low-dimensional state/action and metadata. Recorded camera MP4s remain local
  sidecars and publication assets rather than being included in that archive.
- Replay ramps a compatible calibrated follower to the first pose before
  playback. Pause/resume keeps the current replay frame rather than restarting
  the episode.
- Automatic SpiralDB publication writes completed episode metadata, stream
  semantics, fixed-rate joint samples, actions, and MP4 assets as Arrow
  relations through a trusted ADBC driver. Publication and retry do not depend
  on a browser session. Hosted configurations maintain
  managed per-episode MP4 datasets. Training derives the MP4 frame directly
  from sample index and the declared stream rates, then submits only finite
  frame manifests; Spiral performs frame selection, decode, resize,
  normalization, and Arrow tensor conversion. Rust packs those contiguous
  buffers into Burn tensors without materializing encoded video frames.
  The Studio exposes row-limited, read-only query preview and video retrieval
  through the backend.
- A `--simulate` mode with emulated motors allows calibration, teleoperation,
  recording, and replay without hardware.

**Shipped with a safety boundary:** Studio-managed ACT execution from a durable
Spiral checkpoint. The same control thread that bounds manual, teleoperation,
and replay motion gates policy commands by an unguessable active session. On
macOS rollout prefers Metal and falls back to Accelerate CPU; an inference that
cannot meet the observation safety window is rejected before a new action is
sent.

**Constraints**

- **Hardware:** SO-ARM101 only for now, with leader and follower roles. Live
  calibration supports Feetech protocol-0 motors such as the STS3215.
- **One native training runtime.** Burn training runs in-process under the
  backend supervisor. It uses a dedicated read-only ADBC connection for sample
  queries and the backend's existing trusted connection for checkpoint ingest.
- **Nix flake** provides the development environment and web toolchain (`dx`,
  Tailwind CLI, `wasm-bindgen`, and `wasm-opt`).
- **Safety is enforced by the server.** Enabling torque first copies measured
  position into the goal so an arm cannot snap to stale state. Uncalibrated
  arms cannot accept normalized position commands. Manual, teleoperation,
  replay, and timeline-seek motion use bounded ramps. A non-responsive motor
  bus stops dependent modes and ends an active recording instead of writing a
  silent hole.
- **Camera identity is not stable.** The OS-reported identifier may change
  after a replug or reboot. On macOS, missing camera permission is reported
  explicitly instead of presenting a permanently blank preview.
- **The current browser bundle is served by the backend by default** on
  `127.0.0.1:8080`, with `--bind` for another address. The architectural
  contract permits a separately hosted browser, but authentication, TLS, and
  explicit allowed origins are required before exposing the backend beyond a
  trusted network.
- **The Spiral ADBC driver is trusted native backend code.** Development can
  select it with `--spiral-adbc-driver` or `SPIRALDB_ADBC_DRIVER`. A packaged
  release should bundle an approved driver; the browser never chooses a library
  path.

**Terminology:** *robot instance* (one arm), *leader* / *follower*, *joint*,
*calibration*, *travel*, *torque*, *Hold* / *Release* / *Teleop*, *workspace*
(the directory of robot JSON files), *Studio* (the browser interface),
*workbench* (Live, Data, or Rollout operating panel), *panel* (one registered content
surface), *tile* (a tab group in the layout tree), *dataset* (a collection of
episodes), *episode* / *take* (one recording; *episode* is canonical in data and
wire naming), *Timeline* / *playhead* (the shared episode clock and its current
time), and *Follow Δ* (the gap between measured and commanded joint position).

## Brand Commitments

- The name is **Spiral Robotics**; the web interface is **Spiral Robotics
  Studio**.
- The interface follows the compact, information-dense Notion/Rerun direction
  recorded in `notes.md`, while its panel mechanics use familiar VS Code
  tab-and-tile behavior. Live, Data, Training, and Rollout are the primary activities;
  panels are spatial composition, not a proliferation of navigation modes.
- **`notes.md` is the single source of truth for conflicting changes and must
  not be edited unless explicitly requested.**
- **Distribution:** public open source eventually, but not on the current
  timeline. Keep code and configuration honest for strangers without inventing
  an outward-facing marketing surface, users, or proof.

## Evidence on Hand

Real, in-repo:

- `crates/spiral-robotics-core/assets/so101.urdf` — the shipped robot description.
- `crates/spiral-robotics-panels/` — the reusable panel, tile, docking, persistence,
  keyboard, and motion system used by the Studio.
- `examples/robot.json`, `examples/calibration-input.json` — real configuration
  and calibration input.
- `robots/leader.json`, `robots/follower.json` — a working two-arm setup.
- `crates/spiral-robotics-studio/src/backend/recording/spiral.rs` — the relational episode schema.
- `crates/spiral-robotics-studio/src/backend/robotics/video.rs` — server-owned MP4 capture and
  timeline-aligned segment manifests.
- A detailed `README.md` maintained in the repository.

Deliberately absent, and not to be fabricated: there are **no users,
testimonials, case studies, benchmarks, published performance numbers, pricing,
or deployment/endorsement claims**. The project has not been released.

## Product Principles

1. **The browser observes and requests; the backend owns consequences.**
   Hardware, safety, recording, credentials, data publication, and replay remain
   backend responsibilities regardless of which client is connected.
2. **The wire API is the product boundary.** The current Dioxus UI is one
   client, so correctness cannot depend on browser-local state or component
   behavior.
3. **One clock composes the episode.** State, action, camera media, gaps,
   signals, query selection, and replay meet at the Timeline without erasing
   their natural stream rates.
4. **One file holds the whole robot; completed episodes publish as complete
   units.** Configuration stays portable, while episode retries and destination
   failures never make partial data look complete.
5. **Safety is enforced where it cannot be bypassed.** Every client receives
   the same bounded, calibrated, server-owned motion behavior.
