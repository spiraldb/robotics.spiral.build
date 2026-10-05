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

- **With existing data.** They already have episodes — MCAP datasets on
  Hugging Face, buckets of MCAP files in object storage, exports from other
  rigs — and no place to actually look at, query, or reshape them. They judge
  the Studio on how quickly a pile of files becomes a browsable, queryable,
  curatable dataset.
- **With arms on the bench.** They work with a pair of SO-ARM101 arms — one
  leader and one follower — wired to a Spiral Robotics robot connector, with one or
  more cameras observing the workspace. They judge the Studio on how quickly
  and reliably "arms on the bench" becomes synchronized, queryable episodes,
  and on whether the system stays predictable once physical motion begins.

Studio may run on `robotics.spiraldb.com` or on the engineer's machine; the
robot connector runs on the machine wired to the arms, and the browser driving
them is usually on that machine too.

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
  is the built-in producer. Ingestion from external sources — MCAP
  datasets and object stores full of recordings — is a peer producer and, for
  many users, the primary one.
- **The data core** is where the user spends their time: the episode catalog,
  the Timeline as one synchronized clock over every stream, SQL query, and
  curation — selecting, transforming, and deriving the datasets worth
  training on.
- **Consumers** put the core to work: training a policy, rolling a checkpoint
  out on real hardware, and backing up the complete project.

The full hardware loop — connect, calibrate, teleoperate, record, train,
run a policy — is one path through the system rather than the system's spine.
Vision-conditioned ACT training stores a native artifact, and Rollout loads,
supervises, measures, and applies it through the follower's bounded control
path. Training and Rollout are parked until Spiral decodes MCAP video into
training images: starting a run is refused, and Rollout runs existing
checkpoints.

Success is an ML engineer pointing the Studio at data — theirs or the
world's — understanding any episode against one synchronized clock, curating
a training set with queries instead of scripts, and training from it without
writing glue code or operating a second data stack.

## Positioning

- **The data core is the center of gravity.** Hardware collection is one
  producer among several; query, curation, training, rollout, and export are
  consumers. The Studio competes as the *database-backed* robot-data
  workbench: viewers show files, the Studio owns a queryable project.
- **Two trusted Rust servers, one portable browser frontend.** The Studio
  backend owns ingestion and data services, holds model-provider credentials,
  and relays the browser's provider requests. The robot connector owns
  hardware, safety, recording, and media encoding next to the arms. The
  browser is an interchangeable operating surface over both; it never holds
  service credentials, and it drives hardware only through the robot
  connector's bounded commands.
- **Rust owns the system boundary.** The motor bus, camera capture,
  calibration, kinematics, server, run supervision, data contracts, and web
  interface are Rust. The selected remote ACT/CVAE
  compute runtime retains Arrow transport and PyTorch policy sessions for
  rollout inference, subject to exact checkpoint-schema compatibility. Rust owns lifecycle,
  cancellation, validation, remote telemetry, and SafeTensors checkpoint
  persistence without linking the Studio server to
  LibTorch.
- **One arm is one robot instance.** Serial port, cameras, motors, and
  calibration live in one revisioned robot document on the robot connector
  that drives the arm. JSON is an explicit per-robot transfer format, not live
  storage.
- **Episode data keeps its natural streams and one clock.** Joint state,
  commanded action, camera media, and recording gaps are separate synchronized
  streams rather than one wide table of duplicated or resampled values. The
  Timeline is the common inspection and control surface for that clock. This
  stream model is also the ingestion target: external channels land as
  streams on one clock, not as a resampled wide table.
- **Robot-specific behavior is a capability, not an assumption.** An episode
  is viewable and queryable on its own. Training compatibility is a
  conditional capability an episode has when its streams match a configured
  robot's contract. Ingested episodes without a
  matching robot remain full citizens of the catalog.
- **SpiralDB owns the queryable data.** Datasets, published curations, and
  checkpoints live in Spiral; settings, curations, and training run records
  live in Studio's local SQLite. Each completed Live episode is first saved as
  one immutable MCAP file on the robot connector; the browser then carries it
  to Studio, which imports it as a native Blob into the selected Dataset, and
  only then has the robot connector forget it. Live lists takes still waiting
  and stores them on request. Backend bootstrap pointers
  select the API endpoint, SpFS endpoint, and project, saved in the private
  local SQLite database; never cached Spiral rows. Studio reaches Spiral as the
  signed-in user, sharing the `spiraldb login` session.
- **The robot description is the single source of truth.** Geometry, joint
  limits, and the rendered model come from URDF rather than duplicated drawing
  constants.

## Operating Context

The deployment boundary is deliberately simple:

```text
Browser frontend ─────────────────────────────┐
      │ HTTP commands + frontend reads          │ HTTP commands, reads, MJPEG
      ▼                                         ▼
Studio backend                          Robot connector (next to the arms)
      ├── SpiralDB persistence and          ├── robot connections and safety
      │   read-only queries                 ├── MCAP recording and H.264 encoding
      ├── recording import                  ├── saved robots and calibration
      ├── curation view publication         ├── bounded follower control
      ├── model-provider relay              └── Arrow IPC / HTTP ──► policy runtime
      │     └── HTTPS ──► provider                                  └── rollout session
      └── policy-training supervision
            └── HTTP ──► training runtime ──► model and optimizer session
```

- **The browser carries data between the two.** It renders the interface,
  WebGL scene, charts, tables, and native video. It talks to Studio and to the
  robot connector it is set to, carries each finished take from the robot
  connector to Studio, and writes the checkpoints Studio trained to the robot
  connector. It never connects to SpiralDB; Studio never connects to the robot
  connector.
- **Studio owns data; the robot connector owns hardware.** Studio owns
  SpiralDB credentials and persistence, external-source credentials and
  ingestion jobs, and model-provider configuration; the browser composes
  provider requests and never sees the credential. The robot connector runs
  near the arms and owns all hardware connections, the control loop, safety
  decisions, camera capture, video encoding, and rollout; it never reaches
  Spiral.
- **The wire API is a product boundary.** The Dioxus browser is the current
  client, not a privileged bypass. Hardware and safety behavior must remain
  correct for any client that uses the API.
- **Network degradation is normal operating context.** Reconnection, stale
  telemetry, and unavailable services must suspend or block consequential
  actions without pretending the physical system is safe merely because the
  browser lost contact.
- **Service settings are backend-scoped.** Secrets are held and used by the
  backend: each browser's Spiral session, and the deployment's provider keys
  (the model provider key, the Hugging Face token, and the Modal token), which
  the relay attaches. The browser receives configuration state and who is
  signed in, never a token.

The physical scene matters: an operator may have one hand supporting an arm
during calibration or teleoperation. Releasing torque lets an arm fall under
its own weight, so controls and consequences must remain reachable and stable.

Policy control is owned by the robot connector while tensor execution is remote.
Existing training checkpoints preserve the native ACT graph and normalization
contract as SafeTensors; rollout requires an exact schema match and sends that artifact and Arrow IPC observations to the
selected compute runtime, then admits returned actions to one session-scoped,
bounded motor control thread. The robot connector stops the session on stale inputs,
motor faults, mode changes, inference failures, or excessive
observation-to-action latency. The browser selects and monitors a rollout but
never becomes the motor-control runtime.

## Capabilities and Constraints

**Shipped today**

- A fullstack Dioxus Studio with six primary operating activities. The
  Studio opens on Data, which leads the top bar; Live, the collection
  surface, closes it after Rollout:
  - **Live** owns recording, arm selection, Hold / Release / Teleop, joint
    control, health, and camera status.
  - **Data** owns datasets and their episodes. A rail lists each dataset's
    MCAP files as chronological episodes; selecting a dataset shows its
    overview, and opening an episode shows it in the shared episode
    inspection: its recorded cameras, plots of chosen numeric fields, and one
    Timeline that lists every stream with its message density. Curation
    shows episodes through the same inspection. Import opens as a peer
    tab.
  - **Curation** owns graphs rooted at one dataset whose edges carry episode
    intervals: measures (Break down subtasks, Find similar, Group similar,
    Score progress) add properties or cut intervals, Score progress and Find
    similar keeping what passes their own threshold and Group similar the
    categories it keeps, and Selects keep the episodes whose recorded facts
    pass their conditions. A path compiles to one read-only
    Spiral statement. Each measure previews its own step over the rows its
    parent's preview holds, from the first 100 episodes. Every node but the
    dataset and Break down is an output, and Publish turns the ones chosen
    into named catalog views with one row per kept interval. Every node compiles, the model
    judgments to `spql.ai` functions. Graphs persist in the Studio SQLite
    `curations` table with optimistic revisions and travel in project
    bundles. See `docs/curation.md`.
  - **Training** (parked) retains remote HTTP runtime selection, vision ACT
    configuration, historical learning/resource telemetry, and checkpoint
    inspection/downloads. Starting MCAP camera training is disabled until
    Spiral supports native video decoding into training images; the worker
    also rejects these dataset jobs explicitly.
  - **Rollout** (parked with Training) owns checkpoint selection, fixed-follower readiness, simple
    action-chunk and control-rate settings, the live 3D scene, every follower
    camera recorded for the policy, authoritative start/stop lifecycle, and
    measured native inference evidence.
  - **Settings** owns robot setup (the robot connector's address, ports,
    camera assignment, calibration, add/import/delete), backend data
    destinations, and complete-project backup/restore. Setup is not mixed into
    the live operating flow.
- The external [`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench) crate supplies
  tabbed tiles, horizontal and vertical splits, drag docking with five-zone
  previews, split resizing, keyboard equivalents, session-state panel
  reconciliation, and compact shared chrome. Live, Data, Curation, Training, and Rollout keep independent session
  layouts. Live defaults to a one-third workbench beside a two-thirds
  visualization with movable Scene, camera, and Timeline panels. Data defaults
  to a 21% Datasets rail beside one subject panel; an open episode's
  inspection nests its own workbench of cameras, Scene, Plots, and Timeline,
  arranged for the room it is given.
- Four robot connector motion modes: **Idle**, **Manual**, **Teleoperation**, and
  **Policy**. The Studio presents their operator-facing form as **Hold**
  (torque engaged and joints writable), **Release** (torque off and telemetry
  read-only), and **Teleop** (leader mapped to follower).
- A live 3D scene drawn from URDF, live MJPEG camera panels, and one stable
  stream connection per assigned camera.
- Episode recording during Teleop has server-owned start, pause, resume, stop,
  and discard. Pause/resume preserves one episode identity and elapsed clock,
  excluding paused time. Pause ends the encoder segment without publishing an
  incomplete Dataset member. Final stop saves one immutable MCAP file holding
  joint observations/actions, metadata, and H.264 camera packets, then imports
  it into the configured Spiral project and target Dataset.
- One canonical **Timeline** in Live follows capture. Completed episodes can
  be scrubbed to synchronize recorded camera frames, recorded 3D pose,
  signals, and the playhead. Browser WebCodecs decoding is owned by Rust/WASM;
  camera timestamps follow capture time and seeks begin at a preceding
  keyframe. Scrubbing is visual only and never moves the follower.
- Dataset semantics are explicit: one native Dataset contains many MCAP
  episodes, and Studio never rewrites or deletes one. Calibration changes
  affect future recordings; existing recordings retain their captured
  normalized values. Historical degrees require a calibration snapshot.
- Settings imports and exports the active project as a versioned
  `.spiral-project` bundle (format 4), including original MCAP bytes, Dataset
  names (including empty datasets), checkpoints, and Studio's local records.
  Restore rebuilds native Dataset populations with Studio state in one
  transaction; other bundle formats are refused.
- Native recording import writes a Blob source row, creates the filtered
  source view and target Dataset when needed, and refreshes the population in
  one transaction. An identical recording key and byte payload is an
  idempotent retry; different bytes at that key are rejected. There is no
  separate recording registry, MP4 container, or per-episode video Dataset.
  The backend exposes bounded read-only query preview and H.264 retrieval;
  camera reads reject more than 18,000 packets or 64 MiB of encoded video.
- **Data ownership.** `dataset_source` holds recording sources,
  `dataset_source_<name>` views select each dataset's rows, `dataset_<name>`
  native Datasets reference those views through their definitions,
  `dataset_answers_<name>` tables hold the model answers curations stored
  about each dataset's episodes, and `studio_training_checkpoints` holds
  model artifacts. Rollout's exact
  schema check rejects checkpoints written under another storage schema.
  Studio's own records live in its local SQLite.
- **Native MCAP ingestion.** Data imports MCAP recordings from a public
  Hugging Face repository into one native Spiral Dataset. Source inspection
  resolves a revision and previews files, size, destination, and warnings
  before import. The backend uses `spql.mcap.dataset` and `REFRESH`; there is
  no channel-to-table conversion or parallel Studio dataset row.
- Product simulation is intentionally absent until it can be introduced as a
  separate, principled system rather than another storage or runtime mode.

**Shipped with a safety boundary:** Studio-managed ACT control from a durable
Spiral checkpoint. The same control thread that bounds manual and
teleoperation motion gates policy commands by an unguessable active session. The
selected remote runtime chooses its PyTorch device; an inference that cannot
meet the observation safety window is rejected before a new action is sent.

**Committed direction — confirmed, not yet built**

These are the accepted next moves of the data-core pivot. None of them ship
today, and nothing below may be presented as existing behavior:

- **More ingestion sources.** Local-file uploads and object-storage import
  interfaces extend the shipped native MCAP ingestion path.
- **Training on curated data.** Training reads a published curation view
  and resamples its intervals. Curation publishes the views; Training does
  not read them yet.
- **Generalized episodes.** Episode schema loosened so robot-specific
  semantics (arms, follow error) are optional capabilities rather
  than required fields.

**Constraints**

- **Desktop-only interface:** Studio has a 1024 CSS-pixel minimum viewport and
  always renders the desktop panel tree. It does not maintain mobile
  breakpoints, alternate narrow-screen navigation, touch gestures, or
  touch-target overrides.
- **Hardware:** SO-ARM101 only for now, with leader and follower roles. Live
  calibration supports Feetech protocol-0 motors such as the STS3215.
- **One selected policy compute endpoint.** The configured HTTP runtime
  retains Arrow transport and ACT model inference for schema-compatible
  checkpoints. MCAP dataset training jobs remain unavailable pending native
  Spiral image decoding. Studio owns queries, run history, checkpoint ingest,
  and bounded hardware control.
- **Deployment-provided model provider.** The deployment sets the OpenRouter
  key as `OPENROUTER_API_KEY` in the backend's environment; for now Spiral
  pays for every colleague's model calls, and no user sees or sets the key.
  `LLM_BASE_URL` is an optional process-environment developer override; with
  only the key, requests go to OpenRouter's OpenAI-compatible endpoint. A
  custom endpoint must use HTTPS, except localhost may use HTTP. The key is
  used only by the backend: its transport, and the Spiral driver, which
  receives it as the write-only database option `spiral.openrouter_api_key`
  for the `spql.ai` model functions. It never enters browser state, Spiral
  persistence, or provider prompts.
- **Nix flake** provides the development environment and web toolchain (`dx`,
  Tailwind CLI, `wasm-bindgen`, and `wasm-opt`).
- **Standard CI is service- and compute-free.** It lints and tests the Rust
  packages against the pinned standalone ADBC library without contacting a
  hosted project, runs the frontend's WASM tests in headless Firefox, and
  lints and packages the Python worker without syncing its CUDA stack.
  Hosted-SpiralDB and compute-runtime tests belong in separate integration
  lanes.
- **Safety is enforced by the robot connector.** Enabling torque first copies measured
  position into the goal so an arm cannot snap to stale state. Uncalibrated
  arms cannot accept normalized position commands. Manual, teleoperation, and
  policy motion use bounded ramps. A non-responsive motor
  bus stops dependent modes and ends an active recording instead of writing a
  silent hole.
- **Camera identity is not stable.** The OS-reported identifier may change
  after a replug or reboot. On macOS, missing camera permission is reported
  explicitly instead of presenting a permanently blank preview.
- **The browser bundle is served by the Studio backend**, on
  `localhost:3000` locally and at `robotics.spiraldb.com` when hosted. The
  robot connector listens on `127.0.0.1:7070` by default and accepts any
  origin.
- **The Spiral ADBC driver is trusted native backend code.** Studio loads
  Spiral's standalone shared library through the ADBC driver manager;
  `flake.nix` pins the published archive, and the `local-driver` shell selects
  a current-upstream build. The browser cannot choose or load native code.

**Terminology:** *robot instance* (one arm), *leader* / *follower*, *joint*,
*calibration*, *travel*, *torque*, *Hold* / *Release* / *Teleop*, *Studio*
(the browser interface),
*workbench* (Live, Data, Curation, Training, or Rollout operating panel),
*panel* (one registered content surface), *tile* (a tab group in the layout
tree), *dataset* (a collection of episodes), *episode* / *take* (one kept
multi-stream trajectory, recorded or ingested; *episode* is canonical in data
and wire naming), *producer* (a path that creates episodes: recording or
ingestion), *consumer* (a use of the data core: query, curation, training,
rollout, export), *ingestion*
(backend-owned import of external data into the project), *Timeline* /
*playhead* (the shared episode clock and its current time), and *Follow Δ*
(the gap between measured and commanded joint position).

## Brand Commitments

- The name is **Spiral Robotics**; the web interface is **Spiral Robotics
  Studio**.
- The interface follows a compact, information-dense Notion/Rerun direction,
  while its panel mechanics use familiar VS Code tab-and-tile behavior. Live,
  Data, Curation, Training, and Rollout are the primary activities;
  panels are
  spatial composition, not a proliferation of navigation modes.
- **Distribution:** public open source eventually, but not on the current
  timeline. Keep code and configuration honest for strangers without inventing
  an outward-facing marketing surface, users, or proof.

## Evidence on Hand

Real, in-repo:

- `crates/robot-connector/recording/src/mcap_recording.rs` and
  `crates/robot-connector/src/recording/files.rs` — immutable MCAP packaging
  and the files kept until Studio stores them.
- `crates/backend/storage/src/datasets/sources.rs` — native Blob import,
  Dataset refresh, and idempotent recording retries.
- `crates/robot-connector/src/robotics/video.rs` — H.264 capture with
  capture-time timestamps and parameter-bearing keyframes.
- `docs/datasets.md` — recording, catalog layout, calibration, backup, and
  training limits; physical robot/camera capture still needs a smoke test.
- `crates/frontend/src/activities/curation/path.rs` — the compiler that
  chains each node's step into one statement, with
  `crates/backend/src/curation/manager.rs` owning saved curations and view
  publication.
- A detailed `README.md` maintained in the repository.

Deliberately absent, and not to be fabricated: there are **no users,
testimonials, case studies, published benchmarks, performance guarantees,
pricing, or deployment/endorsement claims**. The project has not been released.
Ingestion beyond public Hugging Face MCAP repositories is a committed
direction, not a shipped capability. No provider credential is shipped as a distributable
product fixture; the deployment sets them in the backend's environment.

## Product Principles

1. **Data is the center of gravity.** Producers put episodes in; consumers
   take datasets out. Hardware collection is one producer, never the front
   door the rest of the product hides behind.
2. **The browser observes and requests; the servers own consequences.**
   Hardware, safety, and recording remain the robot connector's
   responsibilities, and ingestion, credentials, and data persistence Studio's,
   regardless of which client is connected.
3. **The wire API is the product boundary.** The current Dioxus UI is one
   client, so correctness cannot depend on browser-local state or component
   behavior.
4. **One clock composes the episode.** State, action, camera media, gaps,
   signals, and query selection meet at the Timeline without erasing their
   natural stream rates.
5. **A SpiralDB project holds the data, Studio's SQLite its own records, and
   explicit files are transfer formats.** Robot JSON, whole-project bundles,
   and external source files stay portable, while no filesystem spool or retry
   queue becomes a second source of truth.
6. **Robot semantics are capabilities, not assumptions.** An episode earns
   training compatibility by matching a configured robot's contract; without
   a match it remains fully viewable, queryable, and curatable.
7. **Safety is enforced where it cannot be bypassed.** Every client receives
   the same bounded, calibrated, server-owned motion behavior.
