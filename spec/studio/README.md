# Studio workspace

## Outcome

Studio gives an ML engineer one stable browser workspace centered on the
project's robot data: Data is the home, Live produces episodes, Workflow reads
and derives Spiral relations through manual editing or a bounded Assistant,
and Training and Rollout consume them. It keeps feature ownership clear while
allowing visualization panels to be arranged spatially.

## User flow

1. The user opens Studio in Data, or selects Data, Live, Workflow, Training, or
   Rollout from the activity rail.
2. The selected feature route restores its own workspace.
3. The user arranges the panels needed for the task.
4. The status bar and notification center report global state without moving
   the workspace.
5. The user opens Settings from the bottom of the activity rail. Settings
   replaces the activity workspace inside the shell rather than covering it in
   a modal, so the activity rail and status bar remain available.
6. In Settings → SpiralDB, the user may back up or restore the complete active
   project as one portable file.

## Routes and global shell

- **STUDIO-001** Live, Data, Workflow, Training, and Rollout must each have a
  stable URL.
- **STUDIO-002** Each route must own its semantic state, panels, and layout.
- **STUDIO-003** A route change must update every data-driven visualization to
  the selected feature's source.
- **STUDIO-004** A route change must not create empty tiles or move panels into
  another feature.
- **STUDIO-005** Settings must be at the bottom of the activity rail.
- **STUDIO-057** Data must be the first activity in the rail and the default
  route on open, reflecting the data-core story. Live, Training, and Rollout
  follow.
- **STUDIO-006** Studio must not add a top menu or search bar without a confirmed
  use.
- **STUDIO-007** A thin status bar must show connection, mode, rate, active work,
  and problems.
- **STUDIO-008** Recoverable results and errors must use one notification center
  connected to the status bar.
- **STUDIO-071** Feature state must have one authoritative frontend owner at
  the narrowest lifetime that still survives its operations. The shared event
  stream must route Workflow and Assistant snapshots directly into the
  `WorkflowController` nested in `Studio`; `Studio` must not retain a second
  snapshot copy, and the status bar must read the controller projection.
  Values derived from a Dioxus resource, including stable Studio configuration,
  must remain derived rather than being synchronized into another writable
  signal.
- **STUDIO-072** One backend Spiral connection controller must own the active
  endpoint, SpFS endpoint, project, and credentials plus their durable
  bootstrap update. It must perform destination replacement, preference save,
  and project-scoped Training, Rollout, Workflow, and Assistant reload as one
  explicit transition. HTTP adapters submit the settings command and must not
  merge or independently mutate multiple connection copies.
- **STUDIO-073** The active project's qualified Spiral relation names and Arrow
  schemas are one project-scoped resource stored directly in `Studio` state and
  loaded on Studio startup. Data Query and Workflow must consume the resource
  through `Studio` instead of introducing a second catalog context or issuing
  their own catalog requests. Expensive derivatives stay at their narrowest
  useful lifetime: Data Relations owns its combined exact-count query, table
  selection, and five-row preview; individual panels own only transient
  interaction state.
- **STUDIO-074** `Studio` must be the frontend's only session context. Settings
  and project resources plus Data, Workflow, Training, Rollout, and Timeline
  controllers must be nested under that umbrella rather than published as
  sibling contexts. Route-only and component-only interaction state must stay
  at the mounted route or component that owns its lifetime.

## Panel workspace

- **STUDIO-009** A user must be able to move, resize, split, tab, reorder, and
  close supported panels.
- **STUDIO-010** Dragging must show a stable docking preview.
- **STUDIO-011** Live and Data must keep independent layouts.
- **STUDIO-012** Panel layout changes last for the current session only and must
  not create browser-local persistence.
- **STUDIO-013** The default desktop split should place a narrow workbench beside
  a visualization area at about a one-to-two ratio.
- **STUDIO-014** Resizing and conditional content must not cause layout shifts,
  flicker, or playhead jumps.

## Scene and visual presentation

- **STUDIO-015** Studio must provide an interactive 3D Scene for supported arms.
- **STUDIO-016** The Scene must support orbit, pan, zoom, framing, and robot
  selection.
- **STUDIO-017** Robot geometry, axes, and limits must come from URDF.
- **STUDIO-018** The current URDF may be fixed to SO-ARM101 while the renderer
  remains description-driven.
- **STUDIO-068** The viewport crate boundary must be a complete-frame and
  semantic-target API. It must not expose URDF, camera, drag, scene, render,
  entity-key, bench-position, or per-operation request types for the frontend
  to assemble.
- **STUDIO-069** Picking and overlay projection must read the exact scene most
  recently composed by the viewport. The frontend must not independently
  rebuild geometry for picking or condition marks.
- **STUDIO-070** Stable viewport configuration must reject empty or duplicate
  robot and camera identities, invalid colours, unknown attachment frames,
  non-finite transforms, and invalid fields of view before drawing. Measured
  telemetry remains tolerant: missing, unknown, or non-finite observations
  must produce a bounded neutral display pose rather than configuration or
  motion authority.
- **STUDIO-019** Controls must look interactive and telemetry must not look
  draggable.
- **STUDIO-020** Repeated labels and implementation explanations must be removed
  when the surrounding structure provides the meaning.
- **STUDIO-021** Values must use the correct unit and only useful precision. A
  single-series chart with its label already visible must show only the
  formatted value in a single-line tooltip.
- **STUDIO-022** Charts must use Charming/ECharts with linear axes unless a
  metric requires another scale.
- **STUDIO-023** Studio must load a pinned ECharts asset from a CDN instead of
  vendoring the library.
- **STUDIO-024** Long charts must provide data zoom through a visible range
  slider. Related charts should share the selected range. Pointer-wheel input
  over a chart must scroll normally and must not change that range.
- **STUDIO-025** Empty, loading, unavailable, and error states must have stable
  dimensions.
- **STUDIO-026** Whole-project import and export must live in Settings →
  SpiralDB. Import must explicitly warn that it overwrites every Studio-owned
  relation in the active project; export must include the full project rather
  than only the selected dataset, including the validated Workflow feature
  registry and all cache relations it declares.

### Viewport crate boundary

The viewport's job is to turn an ordered frame of supported arms into an
interactive 3D bench. It owns the bundled presentation model and its URDF
parser, pose construction, camera and drag state, scene composition, WebGL
resources, picking, and world-to-canvas projection. It does not own Studio
selection, live-versus-recorded source choice, motor-health policy, camera
configuration, user-facing callouts, or panel state.

The minimal draw input is one `Frame`: grid visibility; each arm's stable ID,
accent, optional observed `ArmPose`, and zero or more configured camera mounts;
and semantic `Interaction`/`Condition` decoration for targets present in that
frame. Runtime steering is sent directly to the long-lived `Viewport` as drag
samples or named `Command`s. The frontend receives only semantic `Target`s and
canvas-local anchors, never renderer keys or geometry.

`Frame` is an incrementally composed, always-valid value. Its mutators validate
stable configuration at the point it enters the crate and return `FrameError`;
appearance for a target absent from the frame is not recorded. Arm ordering
derives centered bench positions, so an out-of-range position cannot be
represented. `ArmPose` has private storage and neutralizes unusable telemetry
in its constructor. The bundled description is a crate invariant validated by
tests rather than a runtime choice repeated by every caller.

After composition, every drawable shape must resolve to one semantic target;
all shells controlled by the same joint must share it. A successful draw makes
that composed scene authoritative for subsequent pick, anchor, and framing
queries. Unit tests cover input rejection, bounded telemetry, camera commands,
scene-to-target round trips, centered placement, URDF parsing/posing, and tool
position. Browser verification remains responsible for WebGL initialization,
resizing, pointer integration, and visible overlays.

The Rust API favors a stateful controller plus a validated frame builder over
public data bags. Presentation joints and commands are enums; pose and frame
fields remain private. Value types implement the comparison, hashing, default,
conversion, display, and error traits their roles require, while renderer-only
types stay crate-private.

- **STUDIO-051** Studio must bind its backend listener before external-service
  probes, camera discovery, project hydration, or saved-robot connection. Its
  backend state contract must distinguish Bootstrapping, Ready, and Degraded;
  the status bar must present that state. State, events, Spiral/runtime
  settings and tests, and camera discovery/preview must remain available while
  bootstrapping. Catalog-dependent routes must return HTTP 503 with a retry
  hint until the catalog is available, and setup snapshots must not perform
  catalog I/O during that interval.
- **STUDIO-052** A setup snapshot must materialize every saved robot from one
  catalog read rather than reloading the robot relation per row. Periodic
  snapshot refresh must run no more often than every two seconds and skip
  missed ticks instead of replaying an overdue polling backlog.
- **STUDIO-053** The unauthenticated hardware-control listener must accept
  loopback addresses by default and reject non-loopback binds unless the
  operator supplies an explicit remote-access opt-in.
- **STUDIO-054** Crates must follow product functionality: `studio` is only the
  product launcher and assets; `frontend` owns the browser interface and its
  interaction components; `backend` is organized by Data, Recording, Robots,
  Training, Rollout, and Workflow capabilities; `api` is the frontend/backend
  wire contract and shared SQL validation; `storage` owns the async database boundary, native Spiral adapter, schemas, SQL
  execution, ingestion, and transfer; `viewport` owns presentation joint maps,
  pose values, and robot geometry; and `hardware` owns motor definitions,
  physical devices, native health verdicts, and fixed-rate control. The retired
  `spiral-robotics-arm`, `spiral-robotics-sql`,
  `spiral-robotics-studio-application`, `spiral-robotics-contracts`,
  `spiral-robotics-studio-backend`, `spiral-robotics-training-http`,
  `spiral-robotics-boundary-build`, `spiral-robotics-core`, and
  `spiral-robotics-ui` crates must not return. A generic root `contracts`
  directory must not combine unrelated compatibility metadata: storage owns
  its schema version and training owns its runtime protocol version and
  supported storage-schema range.
- **STUDIO-055** Dependencies must reflect trust boundaries: frontend depends
  on API and viewport but never backend, storage, or hardware; hardware never
  depends on API; backend composes API, storage, and hardware; storage never
  depends on frontend or backend; and the Studio launcher composes
  frontend with backend only for the native server build. Cross-process work
  must use versioned neutral contracts rather than importing another process's
  implementation.
- **STUDIO-056** Settings must be an in-shell utility panel rather than a modal.
  It must retain its section navigation, fixed action footer, the activity rail,
  and the status bar. A scope-wide destructive
  confirmation opened from Settings may still use the shared blocking dialog.
- **STUDIO-060** Workflow must appear adjacent to Data and before Training in
  the activity rail. Its canvas consumes qualified Spiral relations only;
  Hugging Face, MCAP, and other import producers belong outside Workflow. One
  import may materialize multiple Spiral relations and therefore multiple
  Workflow Table roots.
- **STUDIO-061** Workflow must own an independent four-panel layout scope:
  Assistant and Node tabbed in a 30% left lane, Workflow above Results in the
  right lane.
- **STUDIO-062** Workflow manual editing and execution must remain available
  independently of the Assistant. The Node inspector owns explicit Apply; the
  canvas owns manual Run/Cancel and the controlled Add model action. Add model
  must preserve the existing Query as the candidate cohort, insert the Model
  and a final cache-join Query, and keep both paths visible. Model removal must
  restore the candidate Query as the terminal read. A topology change must auto-layout left-to-right once the cards
  are measurable, including after its tab makes the canvas visible, without
  exposing arbitrary edge drawing or generic node deletion. The Model
  inspector must show the one advertised SigLIP capability; typed feature name,
  description, concept, candidate image field, optional stream applicability,
  authoritative source and source-derived shared cache; bounded feature-cell
  progress; warnings/quarantine; actual device and measured timings; and
  computed/reused state. Untouched definitions use one consolidated completion
  error, and Fit must retain readable 0.70 zoom at the 1024px minimum.
- **STUDIO-064** Workflow Assistant must use the existing 30% Assistant/Node
  lane as one compact one-light thread and composer. It must show secret-free
  product readiness, durable user/assistant/synthetic messages,
  safe structured paragraphs/lists/emphasis/code, streaming/waiting/interrupted
  state, near-bottom auto-follow with an explicit jump recovery, and a compact
  running/complete/failed execution trace whose completed steps collapse. Completed
  graph/run/result tools must expose stale-safe explicit artifact navigation.
  The Assistant has exactly seven typed tools and no sampling or generic
  mutation tool. Periodic frame selection is ordinary read-only Query SQL over
  inspected Spiral relations; that Query feeds the Model as a candidate cohort.
  Restored history and streaming token deltas must not replay through a live
  region; live turn status may announce. Exactly one turn may be active. Send and Stop must remain in the
  composer; the Assistant must not add a second manual Run control or treat
  chat prose as graph, run, or Results truth. Studio must discover its local
  backend-only credential automatically and default the prototype provider/model;
  provider hostnames, model slugs, credential names, and configuration instructions
  must not appear in the product surface. Temporary provider failure keeps the
  stable panel and bounded retry language without implying an active turn.
- **STUDIO-065** Assistant history must be scoped to the active Spiral project,
  hold 1–20 durable conversations without silently deleting an older one, and
  retain at most 80 public messages per conversation. Only the selected
  conversation supplies provider context. New/select must use server scope and
  collection revision, must not mutate the Workflow graph or run, and must be
  idempotent when New is invoked on an already-pristine active conversation.
  They must be
  disabled while a turn or Agent-owned run is active or when the Spiral history
  store is unavailable. Project change or restore replaces rather than merges
  the conversation collection, and named `workflow_agent` events reconcile it
  monotonically without splitting retained tool exchanges. Canvas graph, run
  snapshot, and Results continue to reconcile from authoritative Workflow state
  while a turn streams or automatically resumes after a long run.
- **STUDIO-067** Workflow canvas identity and run state must remain separate:
  Saved/Modified is neutral edit truth, while active/complete/warning/failed and
  neutral cancelled/interrupted states use their semantic roles. First visible
  activation and later panel/window resizes must compact-layout or refit the complete expected five-rank,
  six-node, two-Table lineage at the 1024px desktop minimum. Image-bearing shared query
  Results must preserve the full table and render each decoded image directly
  in its schema cell beside the corresponding score, identity, joint, and other
  values. The table keeps one result metadata strip, type-aware headers,
  aligned numbers, quiet nulls, and contextual image alt text.

## Feature relationships

- Studio-wide actions follow the
  [interaction language](interaction-language.md).
- Live composes [robot control](../robots/control.md) and
  [recording](../recording/README.md).
- Data composes the [episode catalog](../data/README.md),
  [Timeline and replay](../data/timeline-and-replay.md),
  [project data relations](../data/relations.md), and
  [SQL query](../data/query.md).
- Workflow composes the relation-first
  [agentic enrichment workbench](../workflow.md).
- Training and Rollout share the [policy runtime](../training/runtime.md).

## Implementation ownership

- Routes and the global shell:
  [`frontend/shell`](../../crates/spiral-robotics-frontend/src/shell/)
- Route-level composition:
  [`frontend/activities`](../../crates/spiral-robotics-frontend/src/activities/)
- Scene, camera, and Timeline panels:
  [`frontend/panels`](../../crates/spiral-robotics-frontend/src/panels/)
- Panel layout model:
  [`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench) (external crate)
- Shared interaction primitives:
  [`frontend interaction components`](../../crates/spiral-robotics-frontend/src/components/interaction/)
- Backend capability policy and orchestration:
  [`spiral-robotics-backend`](../../crates/spiral-robotics-backend/src/)
- Physical I/O, fixed-rate motor control, and its recording port:
  [`spiral-robotics-hardware`](../../crates/spiral-robotics-hardware/)
- HTTP and compute-runtime adapters:
  [`spiral-robotics-backend`](../../crates/spiral-robotics-backend/)
- Async database access, schemas, SQL, ingestion, and project transfer:
  [`spiral-robotics-storage`](../../crates/spiral-robotics-storage/)
- URDF and rendering:
  [`spiral-robotics-viewport`](../../crates/spiral-robotics-viewport/)

Approved screenshots for this feature belong in [`assets/`](assets/README.md).

## Current gaps

- **STUDIO-057** is not implemented: the code's activity rail leads with Live
  and `Activity::default()` is Live
  ([`frontend/shell/activity.rs`](../../crates/spiral-robotics-frontend/src/shell/activity.rs)).
  Reordering to Data-first is part of the data-core rearrangement.
