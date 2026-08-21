# Product

<!-- impeccable:product-schema 1 -->

## Platform

Desktop web. Studio supports desktop browsers at 1024 CSS pixels and wider.
Mouse and touchpad are the primary pointer inputs, with keyboard alternatives
for workspace operations. Mobile layouts and touch-specific interactions are
outside the product boundary.

## Users

The primary user is an **ML engineer working with robot data**. Their real job
is turning piles of trajectories into policies: understanding what data they
have, deciding what is usable, shaping it into training sets, and evaluating
the behavior that comes out. Spiral Robotics is the data layer they need to
stop fighting.

That engineer arrives in one of two postures, and both are first-class:

- **With existing data.** They already have episodes — LeRobot datasets on
  Hugging Face, buckets of MCAP files in object storage, exports from other
  rigs — and no place to actually look at, query, or reshape them. They judge
  the Studio on how quickly a pile of files becomes a browsable, queryable,
  curatable dataset.
- **With arms on the bench.** They work with a pair of SO-ARM101 arms — one
  leader and one follower — wired to the Spiral Robotics backend, with one or
  more cameras observing the workspace. They judge the Studio on how quickly
  and reliably "arms on the bench" becomes synchronized, queryable episodes,
  and on whether the system stays predictable once physical motion begins.

The browser may be on the same machine as the backend or elsewhere on the
network.

Secondary, and not yet a design target: other SO-101 owners and external
dataset owners who will arrive when the project is opened. See
**Distribution** below.

## Product Purpose

Spiral Robotics Studio is **the place where robot data lives**: one SpiralDB
project where episodes are viewed against a synchronized clock, queried with
real SQL, and shaped into training sets — regardless of whether the Studio
recorded them or ingested them from somewhere else.

The durable shape of the product is a **data core with producers and
consumers**:

- **Producers** put episodes into the project. Recording with connected arms
  is the built-in producer. Ingestion from external sources — LeRobot
  datasets, object stores full of MCAP files, portable episode bundles — is a
  peer producer and, for many users, the primary one.
- **The data core** is where the user spends their time: the episode catalog,
  the Timeline as one synchronized clock over every stream, SQL query, and
  curation — selecting, transforming, and deriving the datasets worth
  training on.
- **Consumers** take datasets and episodes out of the core: training a
  policy, rolling a checkpoint out on real hardware, replaying an episode,
  and exporting portable bundles.

The full hardware loop — connect, calibrate, teleoperate, record, train,
run a policy — remains shipped and supported, but it is one path through the
system rather than the system's spine. Vision-conditioned ACT training stores
a native artifact, and Rollout loads, supervises, measures, and applies it
through the follower's bounded control path.

Success is an ML engineer pointing the Studio at data — theirs or the
world's — understanding any episode against one synchronized clock, curating
a training set with queries instead of scripts, and training and replaying
from it without writing glue code or operating a second data stack.

## Positioning

- **The data core is the center of gravity.** Hardware collection is one
  producer among several; query, curation, training, rollout, and export are
  consumers. The Studio competes as the *database-backed* robot-data
  workbench: viewers show files, the Studio owns a queryable project.
- **One trusted Rust backend, one portable browser frontend.** The backend owns
  hardware, safety, recording, ingestion, media encoding, data services, and
  replay. The browser is an interchangeable operating surface that talks only
  to that backend; it never holds service credentials or drives hardware
  directly.
- **Rust owns the system boundary.** The motor bus, camera capture,
  calibration, kinematics, server, run supervision, data contracts, and web
  interface are Rust. Studio's dedicated ADBC connection reads Arrow state,
  action, and Spiral-decoded image tensors, then streams Arrow IPC batches over
  HTTP to a selected remote ACT/CVAE compute runtime. The same runtime owns
  PyTorch policy sessions for rollout inference. Rust owns lifecycle,
  cancellation, validation, remote telemetry, and SafeTensors checkpoint
  persistence without linking the Studio server to LibTorch.
- **One arm is one robot instance in SpiralDB.** Serial port, cameras, motors,
  and calibration live in one versioned configuration row. JSON is an explicit
  per-robot transfer format, not live storage.
- **Episode data keeps its natural streams and one clock.** Joint state,
  commanded action, camera media, and recording gaps are separate synchronized
  streams rather than one wide table of duplicated or resampled values. The
  Timeline is the common inspection and control surface for that clock. This
  stream model is also the ingestion target: external channels land as
  streams on one clock, not as a resampled wide table.
- **Robot-specific behavior is a capability, not an assumption.** An episode
  is viewable and queryable on its own. Replay, physical seek, and
  training compatibility are conditional capabilities an episode has when its
  streams match a configured robot's contract. Ingested episodes without a
  matching robot remain full citizens of the catalog.
- **SpiralDB is the only persistent data path.** Robots, settings, episodes,
  relational Arrow samples, MP4 assets, saved workflows and runs, training runs,
  checkpoints, and rollout state are written and read through one shared ADBC
  repository. There is no filesystem staging, recovery spool, in-memory
  database mode, or second connected dataset. The selected API endpoint, SpFS
  endpoint, and project are backend bootstrap pointers. Settings may change
  the project across an idle live cutover; a service-endpoint change is saved
  for the next process because ADBC binds both endpoints when constructing the
  database. The namespace is fixed when the repository is constructed. Saved
  runs retain only their selected dataset row. One private local JSON file
  remembers only bootstrap/auth session state, never project records or cached
  Spiral rows.
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
      ├── SpiralDB persistence and read-only queries
      ├── policy-training supervision and checkpoint persistence
      ├── episode validation and bounded follower control
      └── Arrow IPC / HTTP ──► selected compute runtime
                               ├── in-memory model and optimizer session
                               └── in-memory rollout policy session
```

- **The browser can run anywhere.** It renders the interface, WebGL scene,
  charts, tables, and native video. It connects only to the Spiral Robotics
  backend and does not connect directly to robots or SpiralDB.
- **The backend is the one consequential system.** It normally runs near the
  arms and owns all hardware connections, the control loop, safety decisions,
  camera capture, video encoding, SpiralDB credentials and persistence, and
  replay execution. As ingestion arrives, it also owns external-source
  credentials and ingestion jobs.
- **The wire API is a product boundary.** The Dioxus browser is the current
  client, not a privileged bypass. Hardware and safety behavior must remain
  correct for any client that uses the API.
- **Network degradation is normal operating context.** Reconnection, stale
  telemetry, and unavailable services must suspend or block consequential
  actions without pretending the physical system is safe merely because the
  browser lost contact.
- **Replay never drives from a live database stream.** The backend obtains and
  validates the SpiralDB trajectory before motion, then executes through the
  same bounded control path used by every physical mode.
- **Service settings are backend-scoped.** Secrets are stored and used by the
  backend. The browser receives configuration state, not stored tokens.

The physical scene matters: an operator may have one hand supporting an arm
during calibration or teleoperation. Releasing torque lets an arm fall under
its own weight, so controls and consequences must remain reachable and stable.

Studio policy control is backend-owned while tensor execution is remote.
Training exports the exact native ACT graph and normalization contract as
SafeTensors; rollout sends that artifact and Arrow IPC observations to the
selected compute runtime, then admits returned actions to one session-scoped,
bounded motor control thread. The backend stops the session on stale inputs,
motor faults, mode changes, inference failures, or excessive
observation-to-action latency. The browser selects and monitors a rollout but
never becomes the motor-control runtime.

## Capabilities and Constraints

**Shipped today**

- Three CLIs — `spiral-robotics-find-port`, `spiral-robotics-find-cameras`, and
  `spiral-robotics-calibrate` — cover the Studio’s hardware setup flow for scripting and
  machines without a browser.
- A fullstack Dioxus Studio with five primary operating activities:
  - **Live** owns recording, arm selection, Hold / Release / Teleop, joint
    control, health, and camera status.
  - **Data** owns the SpiralDB episode catalog, replay/delete actions,
    selected-dataset episode import/export, and guarded read-only Spiral SQL.
  - **Workflow** owns relation discovery and a persisted, revision-safe
    Table → Query → Result editor over existing qualified Spiral relations. The
    backend runs bounded read-only SQL, publishes run/cancel state through the
    shared event stream, restores saved graphs and results after reload, and
    rejects Model nodes until Stage 3. Workflow has no ingestion controls,
    model execution, or agent chat today.
  - **Training** owns random anchor and camera/action sampling through the
    Spiral ADBC/Arrow path, remote HTTP runtime selection, vision ACT
    configuration, learning/resource telemetry, graceful cancellation, and
    Spiral checkpoints.
  - **Rollout** owns checkpoint selection, fixed-follower readiness, simple
    action-chunk and control-rate settings, the live 3D scene, every follower
    camera recorded for the policy, authoritative start/stop lifecycle, and
    measured native inference evidence.
  - **Settings** owns robot setup (ports, camera assignment, calibration,
    add/import/delete), backend data destinations, and complete-project
    backup/restore. Setup is not mixed into the live operating flow.
- A reusable `spiral-robotics-panels` crate supplies tabbed tiles, horizontal and
  vertical splits, drag docking with five-zone previews, split resizing,
  keyboard equivalents, session-state panel reconciliation, and compact shared
  chrome. Live, Data, Workflow, Training, and Rollout keep independent session
  layouts. The default desktop composition is a one-third workbench beside a
  two-thirds visualization, with Scene/Query, camera panels, and Timeline as
  movable panels rather than fixed slots.
- Four backend motion modes: **Idle**, **Manual**, **Teleoperation**, and
  **Replay**. The Studio presents their operator-facing form as **Hold**
  (torque engaged and joints writable), **Release** (torque off and telemetry
  read-only), and **Teleop** (leader mapped to follower).
- A live 3D scene drawn from URDF, live MJPEG camera panels, and one stable
  stream connection per assigned camera.
- Episode recording during Teleop with server-owned start, pause, resume, stop,
  and discard. Pause/resume preserves one episode identity and records explicit
  time gaps. Assigned cameras can be encoded by the backend into browser-
  playable MP4 segments with episode-clock offsets. Paused checkpoints and
  completed recordings commit directly to the configured SpiralDB project.
- One canonical **Timeline** in both Live and Data. During capture it follows
  new joint and camera data. A paused or completed episode can be scrubbed to
  synchronize recorded camera frames, historical 3D pose, signals, and the
  playhead. Physical time-seeking is separate, explicit, server-gated, and
  applied on committed release rather than every pointer move.
- Dataset semantics are explicit: one dataset contains many episodes. An
  episode can be selected, deleted, or replayed on a compatible follower.
  Data transfers the selected dataset's episodes as `.spiral-episodes`;
  Settings imports and exports the complete active project—including binary
  MP4 and checkpoint data—as one versioned `.spiral-project` file.
- Replay ramps a compatible calibrated follower to the first pose before
  playback. Pause/resume keeps the current replay frame rather than restarting
  the episode.
- Direct SpiralDB persistence writes completed episode metadata, stream
  semantics, fixed-rate joint samples, actions, and MP4 assets as Arrow
  relations through a trusted ADBC driver without a browser session or local
  retry spool. Hosted configurations maintain
  managed per-episode MP4 datasets. Training derives the MP4 frame directly
  from sample index and the declared stream rates, then submits only finite
  frame manifests; Spiral performs frame selection, decode, resize,
  normalization, and Arrow tensor conversion. Rust streams those contiguous
  buffers over Arrow IPC; the remote runtime borrows them into PyTorch tensors
  without materializing encoded video frames.
  The Studio exposes row-limited, read-only query preview and video retrieval
  through the backend.
- Product simulation is intentionally absent until it can be introduced as a
  separate, principled system rather than another storage or runtime mode.

**Shipped with a safety boundary:** Studio-managed ACT control from a durable
Spiral checkpoint. The same control thread that bounds manual, teleoperation,
and replay motion gates policy commands by an unguessable active session. The
selected remote runtime chooses its PyTorch device; an inference that cannot
meet the observation safety window is rejected before a new action is sent.

**Committed direction — confirmed, not yet built**

These are the accepted next moves of the data-core pivot. None of them ship
today, and nothing below may be presented as existing behavior:

- **Ingestion as a peer producer.** Backend-owned ingestion jobs bring
  external episodes into the active project: LeRobot datasets (Hugging Face
  or compatible layouts) first because they map nearly 1:1 onto the episode
  schema, then MCAP files from object stores or local upload with an explicit
  channel-to-stream mapping step. The long-term goal is that any well-formed
  MCAP file ingests without manual schema work. See
  `spec/data/ingestion.md`.
- **Data as the home activity.** The activity rail leads with Data and the
  Studio opens there; Live remains the collection surface.
- **Curation as a consumer.** Materializing a query or a curated selection as
  a new derived dataset, so filtering and shaping training sets happens in the
  Studio instead of external scripts.
- **Generalized episodes.** Episode schema loosened so robot-specific
  semantics (arms, follow error, replay) are optional capabilities rather
  than required fields.

**Constraints**

- **Desktop-only interface:** Studio has a 1024 CSS-pixel minimum viewport and
  always renders the desktop panel tree. It does not maintain mobile
  breakpoints, alternate narrow-screen navigation, touch gestures, or
  touch-target overrides.
- **Hardware:** SO-ARM101 only for now, with leader and follower roles. Live
  calibration supports Feetech protocol-0 motors such as the STS3215.
- **One selected compute endpoint.** Studio reads samples through a dedicated
  read-only ADBC connection and sends Arrow IPC tensor batches to the configured
  HTTP endpoint. The remote runtime holds model, optimizer, and active rollout
  policy state; Studio remains responsible for queries, run history, checkpoint
  ingest, and bounded hardware control.
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
- **The Spiral ADBC driver is trusted native backend code.** Studio links the
  pinned Rust driver crate into its backend; the browser cannot choose or load
  native code. Python training workers continue to use the driver's shared
  library through their language-neutral ADBC manager.

**Terminology:** *robot instance* (one arm), *leader* / *follower*, *joint*,
*calibration*, *travel*, *torque*, *Hold* / *Release* / *Teleop*, *Studio*
(the browser interface),
*workbench* (Live, Data, Workflow, Training, or Rollout operating panel),
*panel* (one registered content surface), *tile* (a tab group in the layout
tree), *dataset* (a collection of episodes), *episode* / *take* (one kept
multi-stream trajectory, recorded or ingested; *episode* is canonical in data
and wire naming), *producer* (a path that creates episodes: recording or
ingestion), *consumer* (a use of the data core: query, curation, training,
rollout, replay, export), *ingestion*
(backend-owned import of external data into the project), *Timeline* /
*playhead* (the shared episode clock and its current time), and *Follow Δ*
(the gap between measured and commanded joint position).

## Brand Commitments

- The name is **Spiral Robotics**; the web interface is **Spiral Robotics
  Studio**.
- The interface follows a compact, information-dense Notion/Rerun direction,
  while its panel mechanics use familiar VS Code tab-and-tile behavior. Live,
  Data, Workflow, Training, and Rollout are the primary activities; panels are
  spatial composition, not a proliferation of navigation modes.
- **Distribution:** public open source eventually, but not on the current
  timeline. Keep code and configuration honest for strangers without inventing
  an outward-facing marketing surface, users, or proof.

## Evidence on Hand

Real, in-repo:

- `crates/spiral-robotics-core/assets/so101.urdf` — the shipped robot description.
- `crates/spiral-robotics-panels/` — the reusable panel, tile, docking, session layout,
  keyboard, and motion system used by the Studio.
- `examples/robot.json`, `examples/calibration-input.json` — real configuration
  and calibration input.
- `robots/leader.json`, `robots/follower.json` — a working two-arm setup.
- `crates/spiral-robotics-studio-backend/src/backend/recording/spiral.rs` — the relational episode schema.
- `crates/spiral-robotics-studio-backend/src/backend/robotics/video.rs` — server-owned MP4 capture and
  timeline-aligned segment manifests.
- A detailed `README.md` maintained in the repository.

Deliberately absent, and not to be fabricated: there are **no users,
testimonials, case studies, benchmarks, published performance numbers, pricing,
or deployment/endorsement claims**. The project has not been released. There
is **no ingestion implementation yet**; ingestion is a committed direction,
not a shipped capability.

## Product Principles

1. **Data is the center of gravity.** Producers put episodes in; consumers
   take datasets out. Hardware collection is one producer, never the front
   door the rest of the product hides behind.
2. **The browser observes and requests; the backend owns consequences.**
   Hardware, safety, recording, ingestion, credentials, data persistence, and
   replay remain backend responsibilities regardless of which client is
   connected.
3. **The wire API is the product boundary.** The current Dioxus UI is one
   client, so correctness cannot depend on browser-local state or component
   behavior.
4. **One clock composes the episode.** State, action, camera media, gaps,
   signals, query selection, and replay meet at the Timeline without erasing
   their natural stream rates.
5. **One SpiralDB project holds the complete durable system; explicit files are
   transfer formats.** Robot JSON, selected-dataset episode bundles,
   whole-project bundles, and external source files stay portable, while no
   filesystem spool or retry queue becomes a second source of truth.
6. **Robot semantics are capabilities, not assumptions.** An episode earns
   replay, physical seek, and training compatibility by matching a configured
   robot's contract; without a match it remains fully viewable, queryable, and
   curatable.
7. **Safety is enforced where it cannot be bypassed.** Every client receives
   the same bounded, calibrated, server-owned motion behavior.
