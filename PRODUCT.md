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
  out on real hardware, and backing up the complete project.

The full hardware loop — connect, calibrate, teleoperate, record, train,
run a policy — remains shipped and supported, but it is one path through the
system rather than the system's spine. Vision-conditioned ACT training stores
a native artifact, and Rollout loads, supervises, measures, and applies it
through the follower's bounded control path.

Success is an ML engineer pointing the Studio at data — theirs or the
world's — understanding any episode against one synchronized clock, curating
a training set with queries instead of scripts, and training from it without
writing glue code or operating a second data stack.

## Positioning

- **The data core is the center of gravity.** Hardware collection is one
  producer among several; query, curation, training, rollout, and export are
  consumers. The Studio competes as the *database-backed* robot-data
  workbench: viewers show files, the Studio owns a queryable project.
- **One trusted Rust backend, one portable browser frontend.** The backend owns
  hardware, safety, recording, ingestion, media encoding, and data services.
  It also owns Workflow's OpenAI-compatible Assistant transport,
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
  save, run, and iterate the same bounded relation-first graph. MCAP camera training
  is unavailable until Spiral supports native decoding into training images.
  The legacy registry/MP4 loader is removed. The selected remote ACT/CVAE
  compute runtime retains Arrow transport and PyTorch policy sessions for
  rollout inference, subject to exact checkpoint-schema compatibility. Rust owns lifecycle,
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
  is viewable and queryable on its own. Training compatibility is a
  conditional capability an episode has when its streams match a configured
  robot's contract. Ingested episodes without a
  matching robot remain full citizens of the catalog.
- **SpiralDB owns the queryable project.** Robots, settings, saved workflows,
  training history, checkpoints, and rollout state use the shared ADBC
  repository. Each completed Live episode is first saved and synced as one
  immutable local MCAP file, then imported as a native Blob into the selected
  Dataset. A failed import reports the retained file path; restart recovery
  and a local-file retry UI are not yet available. Backend bootstrap pointers
  select the API endpoint, SpFS endpoint, and project. One private local JSON
  file remembers bootstrap/auth session state, never cached Spiral rows.
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
      ├── MCAP recording and H.264 encoding
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
  camera capture, video encoding, and SpiralDB credentials and persistence.
  As ingestion arrives, it also owns external-source
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
- **Service settings are backend-scoped.** Secrets are stored and used by the
  backend. The browser receives configuration state, not stored tokens.

The physical scene matters: an operator may have one hand supporting an arm
during calibration or teleoperation. Releasing torque lets an arm fall under
its own weight, so controls and consequences must remain reachable and stable.

Studio policy control is backend-owned while tensor execution is remote.
Existing training checkpoints preserve the native ACT graph and normalization
contract as SafeTensors; rollout requires an exact schema match and sends that artifact and Arrow IPC observations to the
selected compute runtime, then admits returned actions to one session-scoped,
bounded motor control thread. The backend stops the session on stale inputs,
motor faults, mode changes, inference failures, or excessive
observation-to-action latency. The browser selects and monitors a rollout but
never becomes the motor-control runtime.

## Capabilities and Constraints

**Shipped today**

- A fullstack Dioxus Studio with six primary operating activities:
  - **Live** owns recording, arm selection, Hold / Release / Teleop, joint
    control, health, and camera status.
  - **Data** owns datasets and their episodes. A rail lists each dataset's
    MCAP files as chronological episodes; opening one shows its recorded
    cameras and plots of chosen numeric fields on one shared Timeline that
    lists every stream with its message density. Import opens as a peer tab.
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
    envelope and preserves valid current-format history. Unsupported or unreadable
    history is preserved and reported as unavailable; conversation mutations
    remain blocked. Its durable selected thread safely renders compact structured text
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
  - **Curation** owns saved recipes that judge one dataset's episodes for
    training: builtin episode statistics plus declared measurements (camera
    brightness, camera score, a folded topic field, or read-only SQL), rules
    of the form *if fact · operator · value then exclude as reason*, and
    transforms (camera colour profile, camera resize, time trim) recorded for
    the kept episodes. A recipe compiles to one read-only Spiral query with
    positional aliases and runs through the same native executor and run
    polling as Workflow; the sheet lays every episode out as a tile, kept in
    full ink and excluded dimmed under the removing rule's words, with exact
    totals even when the listed roster is bounded, and docks the selected
    episode's evidence beside the tiles. Recipes persist in the
    Studio SQLite `curations` table with optimistic revisions and travel in
    project bundles. Kept episodes are not yet materialized as a dataset.
  - **Training** retains remote HTTP runtime selection, vision ACT
    configuration, historical learning/resource telemetry, and checkpoint
    inspection/downloads. Starting MCAP camera training is disabled until
    Spiral supports native video decoding into training images; the worker
    also rejects these dataset jobs explicitly.
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
  to a 21% Datasets rail beside the episode workspace and a full-width
  Timeline.
- Four backend motion modes: **Idle**, **Manual**, **Teleoperation**, and
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
  episodes. An episode can be selected or deleted. Deletion removes its
  source row and refreshes the Dataset while retaining the local MCAP file.
  Calibration changes affect future recordings; existing recordings retain
  their captured normalized values and are never rewritten. Historical degrees
  require a calibration snapshot.
- Settings imports and exports the active project as versioned
  `.spiral-project` format 2, including original MCAP bytes, Dataset names
  (including empty datasets), checkpoints, and Studio-owned Workflow caches.
  Restore rebuilds native Dataset populations with Studio state in one
  transaction. Format-1 backups require the old application.
- Native recording import writes a Blob source row, creates the filtered
  source view and target Dataset when needed, and refreshes the population in
  one transaction. An identical recording key and byte payload is an
  idempotent retry; different bytes at that key are rejected. No separate
  recording registry, MP4 container, or per-episode video Dataset remains.
  The backend exposes bounded read-only query preview and H.264 retrieval;
  camera reads reject more than 18,000 packets or 64 MiB of encoded video.
- **Data ownership.** Root `studio_*` tables hold application state.
  `dataset_source` holds recording sources, `dataset_source_<name>` views
  select each dataset's rows, and `dataset_<name>` native Datasets reference
  those views through their definitions. Storage schema 11 replaces the old
  container layout. Schema-10 checkpoints remain inspectable/downloadable;
  rollout's exact schema check rejects them under schema 11.
- **Native MCAP ingestion.** Data imports MCAP recordings from a public
  Hugging Face repository into one native Spiral Dataset. Source inspection
  resolves a revision and previews files, size, destination, and warnings
  before import. The backend uses `spql.mcap.dataset` and `REFRESH`; there is
  no channel-to-table conversion or parallel Studio dataset row.
  LeRobot/Parquet/MP4 dataset import is removed.
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
- **Data as the home activity.** The activity rail leads with Data and the
  Studio opens there; Live remains the collection surface.
- **Curated datasets.** Materializing a curation's kept episodes, with their
  recorded transforms, as a new derived dataset. The Curation activity ships
  the recipe, the judgement, and the sheet; the derived dataset does not
  exist yet.
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
  and bounded hardware control. Workflow enrichment
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
  arms cannot accept normalized position commands. Manual, teleoperation, and
  policy motion use bounded ramps. A non-responsive motor
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
*workbench* (Live, Data, Workflow, Curation, Training, or Rollout operating panel),
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
  Data, Workflow, Curation, Training, and Rollout are the primary activities;
  panels are
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
- `crates/backend/storage/src/episodes/mcap_recording.rs` and
  `recording_file.rs` — immutable MCAP packaging and local publication.
- `crates/backend/storage/src/datasets/recording_import.rs` — native Blob
  import, Dataset refresh, and idempotent recording retries.
- `crates/backend/src/robotics/video.rs` — server-owned H.264 capture with
  capture-time timestamps and parameter-bearing keyframes.
- `docs/recording-mcap.md` — recording, playback, calibration, backup, and
  training limits. Native import/retry/playback/delete checks and browser seek
  pixel tests passed; physical robot/camera capture still needs a smoke test.
- `crates/backend/src/workflow/model.rs` — the
  pinned SigLIP runner, SQL-candidate contract, source-derived feature cache,
  Studio feature registry, atomic publication, bounded pipeline, quarantine,
  cancellation, and cross-workflow reuse.
- `crates/backend/src/curation/sql.rs` — the recipe-to-SQL compiler and
  verdict reader, with `crates/backend/src/curation/manager.rs` owning saved
  recipes and judged runs.
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
   Hardware, safety, recording, ingestion, credentials, and data persistence
   remain backend responsibilities regardless of which client is connected.
3. **The wire API is the product boundary.** The current Dioxus UI is one
   client, so correctness cannot depend on browser-local state or component
   behavior.
4. **One clock composes the episode.** State, action, camera media, gaps,
   signals, and query selection meet at the Timeline without erasing their
   natural stream rates.
5. **One SpiralDB project holds the complete durable system; explicit files are
   transfer formats.** Robot JSON, whole-project bundles, and external source
   files stay portable, while no filesystem spool or retry queue becomes a
   second source of truth.
6. **Robot semantics are capabilities, not assumptions.** An episode earns
   training compatibility by matching a configured robot's contract; without
   a match it remains fully viewable, queryable, and curatable.
7. **Safety is enforced where it cannot be bypassed.** Every client receives
   the same bounded, calibrated, server-owned motion behavior.
