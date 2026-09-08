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
  is the built-in producer. Ingestion from external sources — MCAP
  datasets and object stores full of recordings — is a peer producer and, for
  many users, the primary one.
- **The data core** is where the user spends their time: the episode catalog,
  the Timeline as one synchronized clock over every stream, SQL query, and
  curation — selecting, transforming, and deriving the datasets worth
  training on.
- **Consumers** put the core to work: training a policy, rolling a checkpoint
  out on real hardware, replaying an episode, and backing up the complete
  project.

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
  replay. It also owns Workflow's OpenAI-compatible Assistant transport,
  credentials, tool loop, and durable project-scoped conversations. The browser is
  an interchangeable operating surface that talks only to that backend; it
  never holds service credentials or drives hardware directly.
- **Rust owns the system boundary.** The motor bus, camera capture,
  calibration, kinematics, server, run supervision, data contracts, and web
  interface are Rust. Workflow runs its one advertised enrichment model as
  pinned SigLIP safetensors through candle inside the backend over SQL-defined
  candidate image cohorts, and atomically publishes Studio-owned source-scoped
  feature caches plus registry provenance. Its Assistant uses backend-owned,
  OpenAI-compatible tool calling to inspect existing relations and propose,
  save, run, and iterate the same bounded relation-first graph. Training's
  dedicated ADBC connection reads
  Arrow state, action, and Spiral-decoded image tensors, then streams Arrow IPC
  batches over HTTP to a selected remote ACT/CVAE compute runtime. That runtime
  owns PyTorch policy sessions for rollout inference. Rust owns lifecycle,
  cancellation, validation, remote telemetry, feature-cache persistence,
  and SafeTensors checkpoint persistence without linking the Studio server to
  LibTorch.
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
      │ HTTP commands + frontend-initiated reads
      ▼
Spiral Robotics backend
      ├── robot connections and safety
      ├── recording and MP4 encoding
      ├── SpiralDB persistence and read-only queries
      ├── pinned Workflow SigLIP enrichment and source-scoped feature caches
      ├── bounded Workflow Assistant and tool supervision
      │        └── HTTPS ──► configured OpenAI-compatible provider
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
  credentials and ingestion jobs. It already owns Workflow's pinned model
  artifact resolution, local candle execution, cancellation, and derived
  relation lifecycle. It also owns Workflow provider configuration, the
  bounded tool loop, durable bounded Assistant conversations, and provider cancellation; the
  browser composes turns and observes secret-free state.
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

- A fullstack Dioxus Studio with five primary operating activities:
  - **Live** owns recording, arm selection, Hold / Release / Teleop, joint
    control, health, and camera status.
  - **Data** owns a directory of actual Spiral tables, views, datasets, and
    native dataset outputs. Objects open in independent tabs, each with
    Inspect, guarded read-only SQL, and Results. Load and Import sit beside
    the object tabs; dataset inspection distinguishes Episodes from Topics.
  - **Workflow** owns relation discovery and a persisted, revision-safe
    Table → Query → optional Model/backfill → final Query → Result editor over existing qualified
    Spiral relations. The backend runs bounded read-only SQL and one executable
    model capability: `siglip-base`, backed by
    `google/siglip-base-patch16-224` at exact revision
    `7fd15f0689c79d79e38b1c2e2e2370a7bf2761ed`. Read-only Query nodes define
    exact candidate cohorts with five stable frame keys plus decoded
    `spql.image.Image` fields. Typed feature definitions then backfill only
    missing applicable cells in a Studio-owned wide cache derived from the
    authoritative source relation. A Studio feature registry binds every
    `_feature` column to its description, concept, applicability, source, and
    pinned producer provenance; the cache and registry publish atomically.
    Compatible cells are reusable across workflows over the same source, while
    different sources cannot collide. Metal is preferred
    where available and an actual CPU fallback is reported rather than hidden.
    The Model inspector owns typed configuration, measured run state, controlled
    insertion/removal, and topology-aware layout. A compact Assistant shares
    the 30% left lane with Node and uses backend-owned OpenAI-compatible tool
    calling to inspect the active catalog, draft and save valid relation-first
    graphs, start them, wait for bounded or long runs, inspect bounded query
    results, and iterate. Periodic frame selection is ordinary read-only Spiral
    SQL authored in a candidate Query, not a special tool or Model setting; the
    pinned application-side bicubic processor still owns SigLIP resizing.
    Up to 20 project-scoped conversations retain 80 public messages each
    without silently erasing older chats; only the selected conversation enters
    provider context. The server persists them in one versioned schema-v2
    envelope and preserves valid current-format history. Because this is a
    prototype, missing-schema, incompatible, future, or unreadable active-project
    history is atomically reset from its exact stored snapshot to one fresh
    conversation; a failed reset leaves history unavailable rather than
    presenting an in-memory fiction. Its durable selected thread safely renders compact structured text
    and exposes bounded graph/run/result receipts as explicit navigation,
    while the canvas, authoritative run snapshot, and Results remain the source
    of truth; the canvas continues to own the manual Run/Cancel controls.
    Image-bearing query results stay in the shared table: each decoded image is
    rendered in its own schema column beside the matching identity, feature,
    and joint values. Compact type-aware headers, numeric alignment, explicit
    nulls, and one result metadata strip keep Data and Workflow on the same
    ordinary result format.
    Workflow still has no ingestion controls. Hugging Face is used here only to
    resolve the pinned model artifacts.
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
- The external [`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench) crate supplies
  tabbed tiles, horizontal and vertical splits, drag docking with five-zone
  previews, split resizing, keyboard equivalents, session-state panel
  reconciliation, and compact shared chrome. Live, Data, Workflow, Training, and Rollout keep independent session
  layouts. Live defaults to a one-third workbench beside a two-thirds
  visualization with movable Scene, camera, and Timeline panels. Data defaults
  to a quarter-width Tables directory beside its object workspace.
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
- One canonical **Timeline** in Live. During capture it follows
  new joint and camera data. A paused or completed episode can be scrubbed to
  synchronize recorded camera frames, recorded 3D pose, signals, and the
  playhead. Physical time-seeking is separate, explicit, server-gated, and
  applied on committed release rather than every pointer move.
- Dataset semantics are explicit: one dataset contains many episodes. An
  episode can be selected, deleted, or replayed on a compatible follower.
  Settings imports and exports the complete active project—including binary
  MP4, checkpoint data, and Studio-owned Workflow feature registry/cache
  relations—as one versioned `.spiral-project` file.
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
- **Native MCAP ingestion.** Data imports MCAP recordings from a public
  Hugging Face repository into one native Spiral Dataset. Source inspection
  resolves a revision and previews files, size, destination, and warnings
  before import. The backend uses `spql.mcap.dataset` and `REFRESH`; there is
  no channel-to-table conversion or parallel Studio dataset row.
  LeRobot/Parquet/MP4 dataset import is removed.
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

- **More ingestion sources.** Local-file uploads and object-storage import
  interfaces extend the shipped native MCAP ingestion path.
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
- **One selected policy compute endpoint.** Training reads samples through a
  dedicated read-only ADBC connection and sends Arrow IPC tensor batches to the
  configured HTTP endpoint. The remote runtime holds ACT model, optimizer, and
  active rollout policy state; Studio remains responsible for queries, run
  history, checkpoint ingest, and bounded hardware control. Workflow enrichment
  is separate: its one pinned SigLIP capability runs locally in the trusted Rust
  backend and does not become another configurable policy endpoint.
- **Backend-scoped Workflow provider.** The trusted backend can obtain
  `LLM_API_KEY` from explicit process state or the ignored, owner-only
  repository-root `.env.local`; the application loads that fallback itself.
  `LLM_MODEL` and `LLM_BASE_URL` are optional developer overrides; with only
  the key, the prototype defaults to Terra through OpenRouter's OpenAI-compatible endpoint. A
  custom endpoint must use HTTPS, except localhost may use HTTP. The key is
  used only by the backend transport and never enters browser state, Spiral
  persistence, or provider prompts. Browser readiness leaves provider and model
  fields empty, and the product never asks an operator to configure them.
- **Nix flake** provides the development environment and web toolchain (`dx`,
  Tailwind CLI, `wasm-bindgen`, and `wasm-opt`).
- **Standard CI is service- and compute-free.** It tests and lints the Rust
  packages that do not require the native SpiralDB backend, and it lints and
  packages the Python worker without syncing its CUDA stack or running worker
  integration tests. Native-backend, live-SpiralDB, and compute-runtime tests
  belong in separate integration lanes.
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
- **The Spiral ADBC driver is trusted native backend code.** During active
  co-development, Studio links the Rust driver crate from SpiralDB's `develop`
  branch at the revision pinned by `Cargo.lock`. Standard CI excludes the
  native backend with selected-package dependency resolution until a separate
  SpiralDB integration lane exists; it does not fetch the private driver or
  connect to a live project. The browser cannot choose or load native code.
  Python training workers continue to use the driver's shared library through
  their language-neutral ADBC manager.

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

- `crates/frontend/viewport/assets/so101.urdf` — the shipped robot description.
- `examples/robot.json`, `examples/calibration-input.json` — real configuration
  and calibration input.
- `robots/leader.json`, `robots/follower.json` — a working two-arm setup.
- `crates/backend/storage/src/episodes/episode_schema.rs` — the relational episode schema.
- `crates/backend/src/robotics/video.rs` — server-owned MP4 capture and
  timeline-aligned segment manifests.
- `crates/backend/src/workflow/model.rs` — the
  pinned SigLIP runner, SQL-candidate contract, source-derived feature cache,
  Studio feature registry, atomic publication, bounded pipeline, quarantine,
  cancellation, and cross-workflow reuse.
- `crates/backend/src/workflow/agent.rs` and
  `crates/backend/src/workflow/provider.rs` — the bounded relation-first tool loop, project-scoped durable
  session, long-run continuation, and OpenAI-compatible streaming transport.
- A detailed `README.md` maintained in the repository.

Deliberately absent, and not to be fabricated: there are **no users,
testimonials, case studies, published benchmarks, performance guarantees,
pricing, or deployment/endorsement claims**. Development tests do retain
measured SigLIP timing and golden-score evidence, but those observations are
not product performance claims. The project has not been released. There is
**no ingestion implementation yet**; ingestion is a committed direction, not
a shipped capability. No provider credential is shipped as a distributable
product fixture; a developer supplies one in the ignored local secret file or
production secret manager, after which provider setup is invisible in Studio.
Development verification has used a private backend-only OpenRouter
configuration for a bounded Terra smoke and configured-provider screenshot;
that evidence is not a release, endorsement, availability guarantee, or bundled
credential. Local mocked OpenAI-compatible transport tests cover streaming
text, fragmented multiple tool calls, bounded 429 retry, malformed and
oversized streams, cancellation, and secret redaction.

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
   transfer formats.** Robot JSON, whole-project bundles, and external source
   files stay portable, while no filesystem spool or retry queue becomes a
   second source of truth.
6. **Robot semantics are capabilities, not assumptions.** An episode earns
   replay, physical seek, and training compatibility by matching a configured
   robot's contract; without a match it remains fully viewable, queryable, and
   curatable.
7. **Safety is enforced where it cannot be bypassed.** Every client receives
   the same bounded, calibrated, server-owned motion behavior.
