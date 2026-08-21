# Studio workspace

## Outcome

Studio gives an ML engineer one stable browser workspace centered on the
project's robot data: Data is the home, Live produces episodes, Workflow reads
and derives Spiral relations, and Training and Rollout consume them. It keeps
feature ownership clear while allowing visualization panels to be arranged
spatially.

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
  than only the selected dataset.
- **STUDIO-051** Studio must bind its backend listener before external-service
  probes, camera discovery, project hydration, or saved-robot connection. Its
  application contract must distinguish Bootstrapping, Ready, and Degraded;
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
- **STUDIO-054** Framework-neutral application policy must live in its own
  crate. Cargo dependencies, not source-text conventions, must prevent that
  crate from importing HTTP, UI, database, hardware, or model-runtime adapters.
- **STUDIO-055** Dependencies must point inward: domain rules are independent,
  application use cases depend only on domain types and injected ports, and
  backend/UI crates adapt those ports to HTTP, hardware, persistence, and
  presentation. Cross-process work must use versioned neutral contracts rather
  than importing another process's implementation.
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
- **STUDIO-062** Workflow's manual graph execution must keep Assistant in an
  explicit unavailable state with no run authority. The canvas owns Apply,
  Run/Cancel, and the controlled Add model action; Model removal must reconnect
  its Table directly to Query and repair that Query's placeholder. A topology
  change must auto-layout left-to-right once the cards are measurable, including
  after its tab makes the canvas visible, without exposing
  arbitrary edge drawing or generic node deletion. The Model inspector must
  show the one advertised SigLIP capability, eligible upstream media fields,
  concept-to-score-column mapping, bounded progress, warnings/quarantine,
  actual device and measured timings, and computed/reused fingerprint state.

## Feature relationships

- Studio-wide actions follow the
  [interaction language](interaction-language.md).
- Live composes [robot control](../robots/control.md) and
  [recording](../recording/README.md).
- Data composes the [episode catalog](../data/README.md),
  [Timeline and replay](../data/timeline-and-replay.md), and
  [SQL query](../data/query.md).
- Workflow composes the relation-first
  [agentic enrichment workbench](../workflow.md).
- Training and Rollout share the [policy runtime](../training/runtime.md).

## Implementation ownership

- Routes and the global shell:
  [`frontend/app`](../../crates/spiral-robotics-studio/src/frontend/app/)
- Route-level composition:
  [`frontend/screens`](../../crates/spiral-robotics-studio/src/frontend/screens/)
- Scene, camera, and Timeline panels:
  [`frontend/panels`](../../crates/spiral-robotics-studio/src/frontend/panels/)
- Panel layout model:
  [`spiral-robotics-panels`](../../crates/spiral-robotics-panels/)
- Shared interaction primitives:
  [`spiral-robotics-ui`](../../crates/spiral-robotics-ui/)
- Framework-neutral application use cases:
  [`spiral-robotics-studio-application`](../../crates/spiral-robotics-studio-application/)
- Fixed-rate motor control and its recording port:
  [`spiral-robotics-studio-hardware`](../../crates/spiral-robotics-studio-hardware/)
- HTTP, camera, persistence, and runtime adapters:
  [`spiral-robotics-studio-backend`](../../crates/spiral-robotics-studio-backend/)
- URDF and rendering:
  [`spiral-robotics-viewport`](../../crates/spiral-robotics-viewport/)

Approved screenshots for this feature belong in [`assets/`](assets/README.md).

## Current gaps

- **STUDIO-057** is not implemented: the code's activity rail leads with Live
  and `Activity::default()` is Live
  ([`frontend/app/activity.rs`](../../crates/spiral-robotics-studio/src/frontend/app/activity.rs)).
  Reordering to Data-first is part of the data-core rearrangement.
