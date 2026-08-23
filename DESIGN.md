---
name: Spiral Robotics Studio
description: A compact, dockable one-light workbench for operating robots, understanding episodes, training policies, and staging fixed-follower rollouts.
colors:
  canvas: "#ffffff"
  surface: "#ffffff"
  sunken: "#f7f7f5"
  viewport: "#f5f5f3"
  camera: "#1f1f1f"
  lift: "rgb(0 0 0 / 0.12)"
  hover: "rgb(55 53 47 / 0.06)"
  line: "#e8e8e6"
  ink: "#37352f"
  ink-soft: "#65645f"
  ink-faint: "#71706c"
  accent: "#0868b8"
  accent-ink: "#ffffff"
  selected: "rgb(8 104 184 / 0.1)"
  danger: "#be423d"
  danger-soft: "rgb(190 66 61 / 0.1)"
  caution: "#7d6a41"
  caution-soft: "#fbf3db"
  good: "#3a7654"
  good-soft: "rgb(58 118 84 / 0.1)"
  arm-1: "#4a90d9"
  arm-2: "#9a6dd7"
  arm-3: "#4dab9a"
  arm-4: "#e0872f"
  axis-x: "#b65b55"
  axis-y: "#49815f"
  axis-z: "#5178a6"
  signal-1: "#2a78d6"
  signal-2: "#eb6834"
  signal-3: "#1baf7a"
  signal-4: "#eda100"
  signal-5: "#e87ba4"
  signal-6: "#008300"
  series-train: "#2a78d6"
  series-validation: "#d9572b"
  series-memory: "#7a58a5"
  series-transfer: "#7560b8"
typography:
  readout:
    fontFamily: "ui-monospace, SF Mono, JetBrains Mono, Menlo, monospace"
    fontSize: "16px"
    fontWeight: 500
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "tabular-nums"
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 600
    letterSpacing: "-0.01em"
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: "normal"
  action:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: "normal"
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI, sans-serif"
    fontSize: "11px"
    fontWeight: 400
    lineHeight: "normal"
  micro:
    fontFamily: "ui-monospace, SF Mono, JetBrains Mono, Menlo, monospace"
    fontSize: "10px"
    fontWeight: 400
    fontFeature: "tabular-nums"
rounded:
  sm: "4px"
  md: "6px"
  full: "999px"
spacing:
  xxs: "2px"
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "12px"
  xl: "16px"
  target: "24px"
  target-transport: "28px"
components:
  action:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  action-solid:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  action-danger:
    backgroundColor: "transparent"
    textColor: "{colors.danger}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  action-link:
    backgroundColor: "transparent"
    textColor: "{colors.ink-soft}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "0"
    height: "{spacing.target}"
  action-icon:
    backgroundColor: "transparent"
    textColor: "{colors.ink-soft}"
    rounded: "{rounded.sm}"
    size: "{spacing.target}"
  action-selected:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.accent}"
  field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  chip:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "0 4px"
  chip-active:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.accent}"
  chip-caution:
    backgroundColor: "{colors.caution-soft}"
    textColor: "{colors.caution}"
  chip-danger:
    backgroundColor: "{colors.danger-soft}"
    textColor: "{colors.danger}"
  readout-cell:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "2px 6px"
  readout-cell-caution:
    backgroundColor: "{colors.caution-soft}"
    textColor: "{colors.caution}"
  take-action:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.action}"
    rounded: "{rounded.sm}"
    padding: "0 8px"
    height: "{spacing.target-transport}"
  episode-item:
    backgroundColor: "{colors.canvas}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    width: "72px"
    height: "36px"
  episode-item-selected:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.ink}"
  panel-tab:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.label}"
    padding: "0 9px"
    height: "32px"
  panel-tab-active:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    padding: "0 9px"
    height: "32px"
  activity-item:
    backgroundColor: "transparent"
    textColor: "{colors.ink-faint}"
    typography: "{typography.micro}"
    padding: "0 4px"
    width: "60px"
    height: "56px"
  activity-item-active:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.accent}"
  notice-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "4px 8px"
---

# Design System: Spiral Robotics Studio

## Overview

**Creative North Star: "The One-Light Control Bench"**

Spiral Robotics Studio is one calm, literal operating workspace. Warm-white panels,
graphite type, pale WebGL, hairline structure, small radii, and tabular data
make it feel like a precise bench utility rather than a dashboard. Camera pixels
alone keep a dark mat for honest letterboxing; application chrome never changes
visual worlds when the operator changes tasks.

The Studio has five primary activities: **Live** for operating hardware and
recording, **Data** for episodes and query, **Workflow** for deriving and
querying Spiral relations, **Training** for configuring and observing policy
runs, and **Rollout** for staging a trained checkpoint against the configured
follower. Everything else is a panel. A
reusable tab-and-tile system lets the operator attach a panel as a tab, split a
tile, resize the result, or move it with the keyboard. This borrows VS Code’s
learned spatial grammar without importing its visual weight: the layout is
powerful, but the chrome remains Notion-like, compact, and quiet.

The **Timeline is the shared clock**, not a footer or a dataset rail. It is
registered in both Live and Data and synchronizes recording, replay, camera
video, historical 3D pose, signals, query selection, and optionally a
safety-gated physical seek. Signals belongs to the episode on that clock. Full
camera images belong in camera panels; robot ownership remains explicit through
arm and role, and actionable camera failures stay inside the affected frame
rather than being duplicated in the Live workbench.

Density is the point, not a side effect. The base face is 12px, ordinary
controls are 24px tall, and most gaps come from a 2/4/6/8px rhythm. Record is
the one 28px control because it may be reached with a hand still on the arm.
The 16px elapsed readout is the largest type in the app. Colour is spent on
selection, safety, connection, identity, and the one primary action in a local
region; structure comes from alignment and one-pixel rules.

Density only reads as precision if the surface holds still. Telemetry arrives
continuously, so live numbers reserve their widest width, status reports the
exception rather than the resting state, and no sentence unfolds under a
changing value. A dense instrument that twitches reads as noise; one that stays
put reads as a bench tool.

**Key Characteristics:**

- One light neutral world across activity rail, panels, WebGL, plots,
  Timeline, settings, and status bar; camera pixels alone use the dark mat.
- Live, Data, Workflow, Training, and Rollout are the primary activities; independent
  content registers as panels instead of growing one-off shell regions.
- A session-owned tab-and-tile workspace supports docking, tabbing, splitting,
  resizing, keyboard operation, and hot-plugged panel reconciliation.
- The default Live/Data desktop composition is one-third workbench and
  two-thirds visualization, with Timeline in the lower 42% and each camera
  visible in its own row of the named camera column. Rollout narrows the
  workbench to 27% and replaces the Timeline slot with one low Inference lane.
- Workflow uses a 30% Assistant/Node tab lane beside a right column with a
  dominant lineage canvas above Results; the initial 66/34 vertical split
  favors the graph until evidence exists. It has its own layout scope.
- Workflow's Model inspector is the truthful execution surface: one advertised
  SigLIP capability; structured feature name, description, concept, candidate
  image field, optional stream applicability, normalized source and shared
  cache; bounded feature-cell progress; warnings/quarantine; actual device and
  timings; and computed/reused state. The compact Assistant adds a durable
  project-scoped conversation selector, fixed thread header, autosizing composer,
  auto-follow/jump recovery, and collapsible terse tool summaries without displacing
  canvas or Results as the authoritative execution surfaces. Periodic frame
  selection is ordinary read-only candidate SQL, not a typed tool or Model
  setting.
- One playhead synchronizes every representation of an episode.
- Compact 12px type, 24px controls, 32px panel tabs, small radii, and hairlines.
- Monospace tabular figures for elapsed time, rates, durations, counts, and IDs.
- Live values use fixed boxes and exceptional status, so telemetry changes no
  surrounding geometry.
- Role blue marks active, selected, primary, and docking-preview states;
  semantic colour is reserved for real state.
- Studio is desktop-only at 1024 CSS pixels and wider. The persisted split tree
  is always the rendered workspace; there is no alternate narrow-screen UI.

## Colors

The palette is warm-neutral paper and graphite, with one quiet role blue and a
small set of semantic, arm, axis, and signal accents.

### Primary

- **Role Blue** (`{colors.accent}`): Active activity edge, active panel-tab
  edge, selection, focused splitters, docking previews, and the single filled
  action a local region exists to support. Its wash (`{colors.selected}`)
  previews a drop without flooding the workspace.

### Secondary

- **Brick Red** (`{colors.danger}`): Destructive actions,
  recording-in-progress, faults, energized torque, and incomplete episodes.
  The soft wash carries live danger without changing geometry.
- **Bench Ochre** (`{colors.caution}`): Simulation, degraded rates,
  approximate media synchronization, calibration blockers, and other conditions
  that require attention without meaning failure.
- **Connection Green** (`{colors.good}`): Small connected, available, or
  synchronized indicators. It does not fill controls or panels.

### Tertiary

- **Arm identity accents** (`{colors.arm-1}` through `{colors.arm-4}`): Bind a
  robot switcher item, viewport legend, and rendered arm. These colours identify
  objects; they do not imply health.
- **Axis accents** (`{colors.axis-x}`, `{colors.axis-y}`, `{colors.axis-z}`):
  Orientation key only.
- **Joint signal slots** (`{colors.signal-1}` through `{colors.signal-6}`):
  Assigned by joint name in kinematic order so the same joint keeps the same
  trace colour across arms. Every plotted value must also be available as text;
  colour never becomes the only way to read a signal.
- **Training series** (`{colors.series-train}`, `{colors.series-validation}`,
  `{colors.series-memory}`, `{colors.series-transfer}`): Stable semantic roles
  across learning and resource plots. CPU utilization reuses
  `{colors.good}`. Legends and text readouts always accompany the traces.

### Neutral

- **Paper** (`{colors.canvas}` / `{colors.surface}`): Shell, panel content,
  tabs at rest, cards, fields, settings, and notices.
- **Recess** (`{colors.sunken}`): Inactive tab bars, grouped controls, compact
  readout cells, and grouped settings navigation.
- **Pale Bench** (`{colors.viewport}`): WebGL and other visualization frames.
- **Camera Mat** (`{colors.camera}`): Camera frames and camera-unavailable
  surfaces only.
- **Hairline** (`{colors.line}`): Panel, tile, settings, table, and control
  boundaries.
- **Graphite ramp** (`{colors.ink}` → `{colors.ink-soft}` →
  `{colors.ink-faint}`): Primary labels, secondary text, then metadata.
- **Hover wash** (`{colors.hover}`): Quiet hover treatment for controls, tabs,
  activity items, and rows.
- **Lift** (`{colors.lift}`): Floating dock compass and compact elevated
  overlays; never resting panels.

### Named Rules

**The One-Light-World Rule.** Application chrome, panel content, the 3D stage,
Timeline, and data tools belong to one light workspace. Only camera pixels use
the camera mat.

**The Spent-Colour Rule.** Colour means active, selected, primary, identity,
connection, caution, or danger. If none applies, use graphite, a tonal step, or
a hairline.

**The Exceptional-Status Rule.** A chip reports an exception; it does not
announce the resting state. A mode or torque chip exists while that condition
is consequential, not merely because the interface has room for another badge.

**The One-Spelling Rule.** Reusable interface colours are role-named
`--tone-*` properties in `crates/spiral-robotics-studio/tailwind.css`, including
semantic training-series roles. The panel crate maps its `--rp-*` roles to
those tokens. Renderer-owned arm and signal colours remain centralized with
the code that paints them; components do not restate their literals.

**The One-Action-Language Rule.** Product surfaces choose a typed action verb
from `spiral-robotics-ui`. The shared layer owns interaction state,
confirmation, focus movement, and target sizing; a feature owns only placement
and domain-specific content.

## Typography

**Display Font:** None; this is an operating surface, not a presentation.

**Body Font:** System UI (`-apple-system`, BlinkMacSystemFont, SF Pro Text,
Segoe UI, sans-serif).

**Label/Mono Font:** `ui-monospace`, SF Mono, JetBrains Mono, Menlo, monospace.

**Character:** The system face is quiet and platform-native. Monospace is
functional: it stabilizes changing values and lets IDs and rates scan as data.

### Hierarchy

- **Readout** (500, 16px mono, 1): Elapsed episode time and nothing else.
- **Title** (600, 12px, -0.01em): Wordmark, panel-empty headings, selected robot
  identity, and compact region titles.
- **Body** (400, 12px): Actions, fields, ordinary values, and sentences.
  **Action** uses the same size at weight 500.
- **Label** (400–500, 11px): Panel tabs, status, section names, blockers,
  control labels, table headers, and supporting copy.
- **Micro** (400, 10px mono): IDs, durations, counts, rates, paths, and compact
  metadata. It is never the only label on an action or physical consequence.

### Named Rules

**The Tabular-Numbers Rule.** Any number that changes in place — elapsed time,
Hz, degrees, duration, frames, counts — uses tabular figures.

**The Still-Numbers Rule.** Tabular figures equalize digit widths, not digit
counts. Every telemetry-fed value also reserves space for its widest expected
reading so nothing beside it moves.

**The 16px-Ceiling Rule.** The elapsed clock is the Studio’s visual maximum.
Panel titles do not become page headings merely because a tile grows.

**The Sentence-Case Rule.** Labels are sentence case. Do not add uppercase
tracked kickers, decorative display faces, or all-caps panel titles.

## Layout

The first viewport is a full-height application shell:

- a 60px activity rail spanning the body, with
  Live/Data/Workflow/Training/Rollout at the top and Settings anchored at the
  bottom;
- the remaining body area owned by one `PanelWorkspace`, without a separate top
  menu row;
- a 20px bottom status bar for connection, environment, activity, recording,
  torque, rate, and issues;
- transient notices floating above the shell rather than inserting a new row.

Live, Data, Workflow, Training, and Rollout are activity changes, not panel
tabs. Each activity registers its own panel set and uses an independent session
layout scope. Switching activity therefore changes both the operating
content and its visualization without moving Live camera tabs into Data, or
vice versa.

The canonical first-open desktop layout is:

```text
┌──────────── one third ────────────┬────────────── two thirds ──────────────┐
│ Live or Data workbench            │ Scene + Query tab  │ Camera tabs       │
│                                   │                    │                   │
│                                   ├────────────────────┴───────────────────┤
│                                   │ Timeline — lower 42%                   │
└───────────────────────────────────┴─────────────────────────────────────────┘
```

Within the visualization’s upper row, the primary panel takes two thirds and
the named camera tile takes one third. This is a starting composition, not a
fixed semantic order. Every registered panel may be attached to another tile
as a tab or docked to an edge to create a horizontal or vertical split.
Splitters are 8px pointer targets that render as one-pixel hairlines; they clamp
ratios so neither side collapses. Split actions may create an explicit empty
tile as a truthful drop target.

Panel identity is stable and unique. A panel cannot appear in two tiles.
Camera panels reconcile by named home when cameras are assigned, removed, or
loaded asynchronously. Content remains keyed by panel ID so moving or tabbing a
camera does not restart an unrelated feed. Layout mechanics, session state,
keyboard behavior, previews, and chrome belong to `spiral-robotics-panels`; the Studio
owns panel titles, content, safety, and domain state.

The Timeline is always registered in both activities, including empty state.
Its default lower position gives the system a consistent clock, but the user may
move it like any other panel. A paused recording remains inspectable in Live;
stopping returns Live to current hardware and confirms that the episode moved to
Data. Data owns completed-episode selection and query results.

Live begins with Control; recording identity, status, and capture transport stay
in the Timeline and global status bar. Robot identity appears once in a compact
switcher. There is no selected-arm toolbar beneath it. Setup is global Settings
→ Robots rather than a collapsible section in Live.

Data states `Dataset → Episodes` explicitly. Episodes owns select, replay, and
delete, plus lossless episode import/export scoped to the selected dataset.
Query is a peer tool within Data; its result joins the selected
episode's Scene as a sibling tab and never unregisters that Scene. The Timeline
keeps the same layout contract. Recorded camera panels come from the selected
episode’s media streams, not the currently connected camera assignments.

Training is curve-first: its first-open composition gives the learning curve
the dominant visualization area, keeps the literal data path beside it, and
places resource lanes below. The run builder remains a peer panel at left, and
the whole activity uses the same session-owned tab-and-tile workspace mechanics
as Live and Data.

Rollout is a compact physical-policy bench, not an ML dashboard. Its first-open
desktop composition uses a 27% workbench beside a dominant live Scene, every
assigned follower camera recorded for the policy, and one low Inference evidence lane. The
workbench contains only checkpoint selection, fixed-follower readiness, simple
execution inputs, and the primary Run policy action; on a tall workbench, that
action area anchors to the bottom. Follower connection, calibration, and camera
state read as one compact line; do not rebuild them as a readiness checklist.

The Studio shell has a 1024 CSS-pixel minimum width. The panel tree, activity
rail, settings navigation, status bar, and Rollout composition do not switch to
an alternate mobile or touch layout below that boundary.

### Named Rules

**The Panel-Owns-Content Rule.** If a surface has independent identity, focus,
or sizing needs, register it as a panel. Do not add another fixed shell region
or let one component mutate another tile’s tabs.

**The Timeline-Is-One-Clock Rule.** Live, Episodes, Query, camera media, scene,
signals, and replay share one playhead contract. Do not create a second timeline
with different geometry or ownership.

**The Desktop-Only Rule.** Design and test the Studio at 1024 CSS pixels and
wider with mouse, touchpad, and keyboard input. Do not add viewport breakpoints,
mobile navigation, touch gestures, or touch-target overrides.

**The Consequence-On-The-Page Rule.** Text describing energize, release, stop,
discard, replay, or physical seek is rendered on the page, never only in a
tooltip.

**The No-Unfolding-Prose Rule.** A live value changes tint, chip, or control
state; it does not insert a sentence that pushes the panel below it.

## Component Ownership & Data Flow

This section defines behavioral ownership, independently of Rust modules,
crates, or file structure. A **panel** is a movable spatial container. A
**component** is the owner of a user behavior, state transition, or rendering
contract. Moving a component's panel changes where it appears, never what the
component means or which behavior it owns.

### Behavioral Component Hierarchy

```text
StudioShell
├── StudioBody
│   ├── ActivityRail                         primary activity selection; Settings access
│   └── PanelWorkspace                       layout mechanics only
│       ├── SemanticWorkbenchPanel
│       │   ├── LiveWorkbench                operate / record semantics
│       │   ├── DataWorkbench                episode / query semantics
│       │   ├── WorkflowInspectorPanels       saved graph / node-edit semantics
│       │   ├── TrainingWorkbench            train / observe semantics
│       │   └── RolloutWorkbench             fixed-follower policy semantics
│       ├── PrimaryVisualizationPanel
│       │   ├── Scene
│       │   ├── QueryResult
│       │   ├── WorkflowCanvas
│       │   ├── TrainingEvidence
│       │   └── EmptyOrLoadingState
│       ├── CameraPanel(s)
│       └── EvidencePanel
│           ├── TimelinePanel                one shared episode-clock contract
│           ├── WorkflowResultsPanel         bounded media-aware SQL results
│           └── RolloutInferencePanel        measured policy-loop evidence
├── StatusBar
└── OverlayLayer                             settings / notices / confirmations
```

`StudioShell` owns the application frame. `ActivityRail` chooses the active
activity and owns the bottom-anchored Settings entry. `PanelWorkspace` owns
tabs, docking, splits, resizing, reconciliation, and session layout state. It
does not know what an episode, camera, robot, query, or playhead means.

The workbench, primary visualization, camera panels, and evidence panel are
peers in the panel registry even when the default layout places the workbench
on the left, visualization on the upper right, and Timeline or Rollout
Inference on the lower right.

### Semantic Intent and Activity Projection

The left workbench is the **semantic surface**: it expresses what the operator
means to inspect or operate. The right side is the **visual consequence** of
that meaning. The workbench never reaches across the layout to construct,
replace, or mutate another panel directly.

One logical **Activity Controller** per activity sits between user intent,
backend state, and rendering. It may be implemented as a reducer or model
rather than as a visible component. It receives:

- activity selection from the Activity Rail;
- semantic intents from the active workbench;
- time and transport intents from the Timeline;
- typed panel-focus intents, such as choosing a camera stream;
- authoritative snapshots, telemetry, and request results from the backend.

It produces one atomic **Activity Projection**:

```text
Workbench intent ──────┐
Timeline action ───────┼──→ Activity Controller ──→ Activity Projection
Panel focus intent ────┤                                  ├── Primary visualization
Backend event ─────────┘                                  ├── Camera panels
                                                          ├── Signal lanes
                                                          └── Timeline or inference evidence
```

An Activity Projection is a coherent description of the complete active
workspace, not a collection of independently mutable panel states. Changing a
Data selection, for example, derives the Scene, optional additive Query result,
recorded camera registry, signal source, Timeline subject, and playhead
together. A Query result may keep an explicitly named episode context or have
no episode context; it never replaces the Scene panel identity, and the Scene,
cameras, and Timeline must all report the same choice.

Each activity owns independent controller state. Switching activities restores
the target activity's semantic selection, projection, time/evidence subject,
and panel layout without letting hidden activity state mutate the visible one.

### Visualization Source Boundary

Scene, Camera, and Signals are data-driven renderers. They receive an explicit
source from the Activity Projection and do not choose a source from ambient
global selection or from the absence of another value.

```text
VisualizationSource
├── Live
│   └── stable stream identity + current frame/state
└── Recorded
    ├── episode identity + requested episode time
    └── Loading | Ready | Unavailable | Failed
```

`Loading`, `Unavailable`, and `Failed` are real recorded-source states. They are
not aliases for Live. When a recorded sample or clip is missing, the panel shows
that recorded-source state and never substitutes current live telemetry or a
live camera frame. This prevents a panel titled for an episode from displaying
the robot's present physical state.

Live camera panels derive from the current Live robot assignments. Recorded
camera panels derive from the selected episode's stream identities. Activating
a camera panel emits that typed live-camera or recorded-stream identity; domain
meaning is never recovered by parsing a panel ID string.

Rollout's Scene and cameras are explicit live sources scoped to the configured
follower: the Scene may show both arms for physical context, while the camera
panels mirror the follower cameras recorded for the policy. Inference
evidence is a separate measured source and stays empty until the backend has
completed a native inference.

### Timeline Interface Contract

There is one canonical Timeline component and one action vocabulary in Live,
Data, review, query-context, loading, and empty states. The Timeline is never
replaced by a second empty-state component with a smaller behavioral boundary.
Unavailable actions remain part of the interface and are disabled with a
specific reason.

Rollout does not own an episode clock, so it deliberately registers Inference
instead of Timeline. That exception does not create another playhead or reuse
episode transport for model execution.

The Timeline owns:

- the visible episode clock and playhead;
- pointer drag and keyboard seek interaction;
- Record, Pause/Resume, Stop, and Discard capture transport;
- one icon-only Play/Pause transport with an accessible action name;
- Follow Live while an episode is growing;
- signal lanes and explicit recording gaps;
- one explicit **Sync robot** option that applies to playback and committed seek.

Every Timeline action carries the identity of the recording or episode it acts
upon. Timeline transport never discovers its target from an unrelated ambient
selection. With **Sync robot** off, Play/Pause and seeking change recorded
visualizations only. With it on, the same Play/Pause control drives the
compatible robot and committed seeks move it through the server-owned safety
path. Turning the option off stops active robot motion; there is no second
Replay/Pause/Resume/Stop transport competing with the Timeline clock.

A committed physical seek may use a bounded one-frame replay inside the
hardware controller. That one-frame consequence never owns preview transport,
changes the Timeline playhead, or replaces the stable toolbar while the arm is
travelling to the requested pose.

Dragging or keyboard-seeking emits preview updates while the pointer or key is
moving, then a committed seek at release or commit. Each accepted playhead
change produces one synchronized projection for the Scene, every recorded
Camera panel, Signals, and the Timeline itself. When **Sync robot** is enabled,
physical motion is sent only for the committed seek.

### State Ownership

| State | Owner | Consumers |
|---|---|---|
| Active activity | `StudioShell` / Activity host | Activity Rail, active controller, Status Bar |
| Live robot and camera selection | Live Activity Controller | Live Workbench, Scene, Live camera registry |
| Live recording and review subject | Live Activity Controller, reconciled with backend truth | Live Workbench, Timeline, recorded preview surfaces |
| Data tool, selected episode, and query | Data Activity Controller | Data Workbench and the complete Data projection |
| Workflow saved graph, local draft, typed feature definitions, source-derived cache metadata, run snapshot, and terminal result | Workflow Activity Controller, reconciled with backend truth | Node panel, Workflow canvas, Results panel, Assistant, Status Bar |
| Workflow Assistant readiness, bounded project-scoped conversations, active selection/turn, and typed tool summaries/receipts | Backend Workflow agent, reconciled by the Workflow Activity Controller | Assistant panel; canvas and Results consume only authoritative Workflow state |
| Rollout checkpoint, execution inputs, fixed follower, and readiness | Rollout Activity Controller, reconciled with backend truth | Rollout Workbench, live Scene, follower Camera, Inference lane |
| Timeline subject, playhead, preview playback, and Follow Live | Timeline Controller within the active activity | Scene, Cameras, Signals, Timeline |
| Panel geometry and active tab | `PanelWorkspace` | Panel chrome and persisted layout only |
| Hardware, recording, replay, storage, and safety state | Backend | Activity Controller through the wire API |

Primary visualization content, camera content, and Timeline content are derived
projection outputs. They are not additional independent sources of semantic
truth.

### Required Interaction Flows

**Select an episode:** `DataWorkbench → SelectEpisode(id) → Data Activity
Controller → Recorded source → Scene + Cameras + Signals + Timeline`.

**Open Query:** `DataWorkbench → ShowQuery(query) → Data Activity Controller →
preserved Scene + active QueryResult sibling tab + explicit episode context or
explicit no-episode context`.

**Apply and run a Workflow:** `Node inspector edits → revision-checked Apply →
Workflow backend → optional pinned SigLIP feature-cache backfill → read-only Query →
run/cancel snapshot events → reconciled Canvas + bounded media-aware Results`,
while browser-owned node geometry and panel focus remain stable. The controlled
Add/Remove model actions rewrite the Table→Query lineage and placeholder as one
topology change; they never expose arbitrary graph editing.

**Ask the Workflow Assistant:** `Composer → backend-owned provider turn →
relation/model inspection → read-only SQL candidate definition → validated
graph save → optional Workflow run → bounded query inspection → durable thread
+ named agent SSE`. The selected conversation alone supplies provider context;
server-scoped create/select changes conversation state without changing the
Workflow graph or run. Text and compact tool summaries stream in the Assistant
lane. When a run exceeds the bounded
wait, the backend publishes its terminal state, appends a synthetic completion,
and resumes the same turn. Saved canvas state, run snapshots, and Results are
authoritative throughout; Assistant activity never creates a browser-local
graph or a second Run control.

**Seek or rewind:** `Timeline → Seek(subject, time, phase) → Activity Controller
→ one synchronized historical projection`. A committed, explicitly armed
physical seek additionally becomes a backend request.

**Receive live telemetry:** `Backend event → Live Activity Controller → Live
projection → Scene + Cameras + Signals + Timeline`.

**Select a rollout checkpoint:** `RolloutWorkbench → SelectCheckpoint(id) →
Rollout Activity Controller → fixed follower + live Scene + first follower
Camera + truthful Inference state`. Run remains disabled until the checkpoint,
follower, camera binding, calibration, and fresh control stream are available;
the backend revalidates the complete model and hardware contract before motion.

### Named Rules

**The Semantic-to-Projection Rule.** Workbenches emit intent; they do not mutate
visualization components. One controller derives the complete visible
consequence.

**The Atomic-Projection Rule.** Primary visualization, cameras, signals, and
Timeline are updated from one coherent activity projection. No surface may
retain an incompatible interpretation of the current semantic selection.

**The Explicit-Source Rule.** A visualization receives Live or Recorded as an
explicit source with explicit loading and failure states. Missing recorded data
never falls back to Live.

**The No-Ambient-Identity Rule.** An action names the episode, stream, robot, or
query it affects. A component never guesses its target from another activity's
selection or from a panel ID naming convention.

**The Timeline-Owns-Time Rule.** Timeline owns the user-facing time operation;
the Activity Controller owns the synchronized projection caused by that
operation; the backend owns any physical consequence.

## Elevation & Depth

The Studio is flat by default. Panel and tile hierarchy comes from tonal steps,
hairlines, active edges, and content mats rather than card shadows. Elevation is
reserved for an object that genuinely floats above the shell.

### Shadow Vocabulary

- **Resting lift** (`0 1px 2px rgb(0 0 0 / 0.05)`): Small controls placed above
  a visualization, such as the view cube.
- **Dock preview lift** (`0 6px 18px rgb(0 0 0 / 0.08)`): The blue docking
  footprint while a tab is dragged.
- **Dock compass lift** (`0 6px 18px rgb(0 0 0 / 0.12)`): The small floating
  five-zone orientation cue.
- **Floating notice** (`0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)`):
  Settings-adjacent popovers and bottom notices only.
- **Inset state** (`inset 0 0 0 1px currentColor`): Selection and disabled
  state, not elevation.

### Named Rules

**The Flat-by-Default Rule.** Resting panels, tabs, cards, controls, Timeline,
and settings do not cast shadows.

**The Inset-State Rule.** Selection and disabled treatments happen inside the
existing box with a wash, edge, or inset ring. State changes never add geometry.

## Shapes

The form language is compact and gently squared: 4px for controls, chips,
episode items, and tile actions; 6px for floating notices and the dock compass;
full circles only for status dots, arm marks, and slider thumbs. Panel and tile
containers remain square so adjacent surfaces meet as one workspace.

Icons are outline geometry with round caps and joins, normally 16px inside a
24px target. Panel split icons describe direction rather than decoration.
Controls remain shallow rectangles; avoid pills, oversized cards, and floating
islands.

## Components

### Actions

Every interactive action is an `Action`, `ActionLink`, `FileAction`,
`ModeAction`, or `Toggle` from `spiral-robotics-ui`. Product code declares the
verb first; visual presentation never changes the verb.

- **Shape:** Compact rectangle, 4px radius, 24px target, and 6px horizontal
  padding. Prominent transport actions use 32px.
- **Presentation:** Quiet, Outline, Solid, Link, Icon, Row, Tab, and Segment are
  presentation variants. A local surface may own geometry but not restate
  hover, focus, selected, disabled, pending, or destructive state.
- **Primary:** Role-blue fill with white text, once per local purpose: Record,
  Run query, Run workflow, Connect, Save, Run training, or Run policy.
- **Danger:** Delete, Discard, Replace, Release, and Stop use the shared red
  treatment. Their consequence determines the confirmation level; the visual
  variant does not.
- **Selected:** `aria-pressed`, `aria-selected`, `aria-checked`,
  `aria-expanded`, and `aria-current` all use the same inset blue state.
- **Focus:** The global accent focus ring remains visible inside dense chrome.
  A confirmation focuses its safe action, Escape cancels, and dismissal returns
  focus to the originating action.
- **Pending:** The initiating action stays in place, names the work in progress,
  exposes `aria-busy`, disables repeat submission, and preserves a failed form
  or confirmation for correction.

#### Action grammar

- **Create** makes a durable entity; **Assign** creates a relationship.
- **Connect** and **Disconnect** change a live device connection.
- **Remove** or **Unassign** detaches a reversible relationship.
- **Close** dismisses a view while preserving its data.
- **Delete** permanently removes a durable entity.
- **Discard** abandons uncommitted work; **Replace** overwrites an existing
  scope.
- **Apply** commits configuration or calibration; **Release** turns off motor
  torque and always states the physical consequence.
- **Sync** reconciles clocks or state; **Retry** repeats failed work. Start,
  Stop, Cancel, Import, Export, Refresh, Navigate, Select, Toggle, Hold, and
  Teleoperate retain their literal meanings. Panel reordering is a workspace
  gesture with pointer and keyboard paths, not a product action purpose.

#### Placement and confirmation

- View controls live in the panel or dialog header. Collection-wide actions
  live in the collection toolbar. Row-local actions stay at the trailing edge.
  Save and Cancel live in the editor footer. Physical-safety actions stay in
  the safety region.
- Recoverable actions happen immediately. Local permanent actions use the
  shared inline confirmation. Scope-wide replacement and physical consequences
  use the shared blocking alert dialog. Native browser confirmation is not part
  of the language.
- Hover may reveal a quiet trailing action only when the same action remains
  keyboard-reachable.

### Chips

- **Style:** 11px, 4px radius, compact horizontal padding, graphite ink, and a
  tonal wash.
- **State:** Blue for active, ochre for caution, red for fault or torque danger,
  and a small green dot for simple availability.
- **Presence:** Per the Exceptional-Status Rule, a chip renders only when its
  condition matters.

### Panel Tabs, Tiles, and Splits

This is the signature component system.

- **Tab bar:** 32px recess surface with a one-pixel lower rule. Tabs are 11px,
  80–224px wide, and horizontally scroll without showing a scrollbar.
- **Active tab:** Paper surface, primary ink, and a 2px role-blue top edge.
  Inactive tabs use soft ink and hover wash.
- **Tile:** Flat paper content frame, minimum 10rem × 5rem on desktop. Empty
  tiles state that they are drop targets and offer a close action.
- **Tile actions:** 24px split-right, split-down, and close controls. They reveal
  by opacity on hover/focus and remain present on non-hover devices.
- **Splitter:** 8px hit target with a 1px rule. Hover, focus, or active resize
  changes only the rule to role blue. Double activation restores a 50/50 split;
  arrows resize, with Shift for a larger step.
- **Keyboard tabs:** Left/Right, Home, and End change the active tab. Alt+Shift
  plus arrows splits/docks; Alt+Shift plus Page Up/Down moves between tiles.
- **Persistence:** Layout mutations store immediately, while pointer resizing
  stores on release. Invalid saved trees fall back to the canonical layout.

### Dock Preview and Motion

- **Preview:** Dragging a tab exposes five zones on each tile. Center attaches
  as a tab; left/right/top/bottom creates a split. The selected zone is a blue
  wash with a blue hairline, inset 4px from its target footprint.
- **Compass:** A 38px paper square with a 6px radius and blue directional mark.
- **Timing:** Split basis changes and tile entry use 140ms with
  `cubic-bezier(.16, 1, .3, 1)`; preview entry uses 120ms with the same curve;
  colour and reveal changes use 100ms ease-out.
- **Resize:** Geometry transition is disabled while a splitter is actively
  dragged so the divider tracks the pointer exactly.
- **Reduced motion:** All panel transitions and entry animations are removed
  under `prefers-reduced-motion`.

### Application Chrome

- **Activity rail:** 60px wide on desktop and the only global navigation
  surface. Live, Data, Workflow, Training, and Rollout stay at the top, in that
  order; Settings stays at the bottom. Active activities use blue ink, a paper
  surface, and a 2px left edge. There is no separate top menu.
- **Status bar:** 20px, 10px type, hairline-separated facts. It owns global
  connection, environment, current activity, recording, holding, rate, and
  issue truth.

### Workbench Sections

- **Robot switcher:** Compact grid of arms with one identity accent and one
  readiness dot per item.
- **Control ownership:** The Control section exclusively owns Hold, Release,
  and Teleop, including the inline Release confirmation. Hold enters Manual for
  the selected arm before energizing it; Release leaves the active mode and
  drops torque. There is no second selected-arm toolbar.
- **Motor default:** Selecting an arm, or opening Live without an object
  selection, selects that arm's first configured motor so its diagnostic detail
  is immediately visible. An intentional camera selection is preserved.
- **Camera ownership:** Camera panels own camera identity, pixels, and failure
  state. Live does not repeat them as a camera list under joint controls.
- **Setup:** Lives in Settings → Robots. Ports, camera assignment, calibration,
  import/export, and deletion do not compete with active motor control.

### Workflow Workbench

- **Relation-first boundary:** Workflow begins with qualified Spiral Table
  relations. Hugging Face datasets, MCAP files, and other external sources are
  import producers owned elsewhere; they never appear as Workflow nodes or
  controls. One import may produce several Table roots.
- **Node grammar:** Table, Model, Query, and Result are the only node kinds.
  A Query defines the Model's candidate cohort; Model cards name the
  source-derived Studio feature cache; a final Query joins candidate/cache and
  other evidence; Result is terminal.
- **Card:** Paper surface, one-pixel hairline, 4px radius, 12px sentence-case
  title, 10–11px detail, and a reserved footer for semantic state. Selection is
  a role-blue inset ring and must remain legible without relying on colour.
- **Canvas:** Pale viewport surface with a subtle dot grid, thin graphite
  lineage edges, no MiniMap, and a typed Fit action. **New workflow** creates a
  reversible unapplied blank draft; Apply persists the empty canvas and clears
  stale Results so the Assistant's later save is visibly the source of every
  node. The blank canvas and Results use matching activation copy rather than
  showing graph-validation jargon. `--df-*` values map to the existing
  `--tone-*` roles rather than forming a second palette.
- **Current truth:** The 30% left lane keeps Assistant and Node as tabs. The
  Assistant shows secret-free product readiness, a durable bounded
  conversation selector, the selected message thread, safe paragraphs/lists/emphasis/inline-code/code-block
  rendering, streaming response state, near-bottom auto-follow with explicit
  jump recovery, and a compact disclosed execution trace that stays collapsed
  during ordinary work. A successful turn labels nonfatal retries as recovered;
  only a terminal failed turn opens the trace for action.
  Completed graph/run/result tools offer stale-safe **View graph**, **View run**,
  or **View results** receipts without moving focus until invoked. The
  autosizing 8,000-character composer keeps Send or Stop as mutually exclusive
  turn actions; Enter sends and Shift+Enter inserts a line. Up to 20 conversations with 80 public messages each are preserved;
  hitting the bound disables New instead of evicting history. New/select are
  also disabled during Agent-owned work or when Spiral history is unavailable,
  but remain usable when only provider availability is absent. New is idempotent
  on an already-pristine active conversation. Studio silently reads its ignored,
  owner-only local credential fallback and applies the prototype Terra/OpenRouter
  defaults; provider/model identities, credential names, and configuration
  instructions never appear in the product surface. An unavailable provider
  leaves the same stable surface with bounded retry language.
  Manual Table→Query→Result and
  Table→candidate Query→Model→final Query→Result graphs continue to run over
  authoritative Spiral relations. The canvas exclusively owns manual
  Run/Cancel and prints every disabled reason on-page; the Assistant may start
  a run only through its bounded backend tool.
- **Agent boundary:** Seven backend tools may list and describe existing
  relations, list the one advertised Model capability, validate/save a complete
  Workflow, start and wait for that Workflow, and inspect at most 50 read-only
  query rows. The graph tool exposes only name, nodes, and edges; Studio injects
  the server-owned workflow identity and revision before validation. There is no
  sampling or generic mutation tool. Periodic selection
  is an ordinary read-only candidate Query using inspected Spiral media and
  `frame_slice` before decode; the existing pinned PIL bicubic processor remains
  the only SigLIP resize path.
  Tool state is summarized from bounded allow-listed output facts such as graph
  revision, run identity/phase, query row counts, and terminal result count;
  raw tool payloads, media/tensors, and provider credentials never appear
  there. Only one
  turn is active, and manual Apply or
  Run waits until it finishes. A long Workflow run moves the turn into an
  explicit waiting state and resumes it from a synthetic backend completion.
  Restarted active turns become Interrupted rather than appearing to resume;
  an incomplete trailing provider tool exchange is removed before Retry.
- **Manual inspector:** The only advertised Model is pinned SigLIP; its
  inspector owns structured `_feature` name, description, concept, candidate
  image field, optional stream IDs, normalized source, and source-derived cache.
  Untouched feature definitions collapse to one actionable completion issue;
  entered invalid fields retain their field-specific errors. Table, Model, and
  Query edits use explicit Apply with revision-conflict recovery.
- **Controlled topology:** Add model preserves the existing Query as a candidate
  cohort, inserts SigLIP plus a generated final cache-join Query, and connects
  candidate→Model, candidate→final Query, and Model→final Query. Remove model
  restores the candidate Query as terminal.
  Topology changes trigger compact left-to-right auto-layout after cards are
  measurable. First visibility also lays out before fitting so a five-rank,
  six-node, two-Table lineage stays complete at the supported 1024px desktop minimum;
  later activation only fits unchanged geometry. Manual Fit also clamps to the
  0.70 readability floor and leaves wider graphs pannable. The canvas exposes no
  arbitrary connection or delete tool.
- **Runtime state:** Cards render authoritative Pending, Validating, Loading,
  Running, Complete, Complete-with-warnings, Failed, Stale, Cancelling, and
  Cancelled states. Model Running uses bounded `completed / total feature cells`, never
  fabricated percentages or ETA. The inspector exposes actual device,
  decode/inference timing, quarantine count/examples, warnings, fingerprint,
  and computed/reused state. A neutral Saved/Modified identity is separate from
  the semantic run badge: active is blue, Complete green, warnings ochre,
  failure red, and Cancelled/Interrupted neutral. New cards use one bounded
  left-to-right reveal when an Assistant save introduces topology. During a
  run, only lineage feeding the active node uses marching dashes; the active
  footer carries a restrained moving wash and real completed/total work uses a
  semantic progress element. All three motion cues stop with the state and are
  disabled under `prefers-reduced-motion`. Reruns and
  failed/cancelled attempts retain prior query rows as visibly stale and expose
  phase-specific recovery. Results reuse Data's bounded media-aware renderer;
  exact `spql.image.Image` values render directly in their schema cells, with
  contextual episode/stream/frame alt text. Image-bearing rows retain ordinary
  table semantics beside their score, identity, time, frame, and joint values;
  compact type-aware headers, aligned numbers, quiet nulls, and one metadata
  strip avoid a duplicate evidence surface.
- **Focus, history, and reload:** Pointer selection or Enter/Space from a focused
  node opens the Node inspector; initial rendering leaves Assistant active.
  Workflow and `workflow_agent` SSE/reconnect updates preserve node geometry and
  never steal panel focus. The backend persists bounded Assistant conversations
  and tool summaries as one server-owned schema-v2 envelope in the active Spiral
  project; only the selected conversation is provider context, while
  create/select leaves graph/run truth unchanged. Valid current schema-v2
  history is preserved. This prototype atomically resets missing-schema,
  incompatible, future, or unreadable history from its exact stored snapshot to
  one fresh conversation; if that reset fails, history remains unavailable and
  conversation mutation stays blocked.
  Switching or restoring a project replaces the visible collection under a new
  server scope instead of merging histories. Unsent Assistant drafts are keyed
  by both server scope and conversation identity, so a cloned/imported
  conversation identity cannot expose another project's draft. A dirty graph draft whose
  base revision is obsolete can only be discarded through the explicit Reload
  saved action.

### Timeline

- **Transport:** Record, Pause/Resume, Stop/Discard, one icon-only Play/Pause,
  **Sync robot**, and the 16px clock occupy one stable toolbar. Record always
  implies Teleop. Play/Pause affects visualizations only until robot sync is
  explicitly enabled.
- **Playhead:** One pointer and keyboard-operable time cursor drives every
  historical surface. Scrubbing updates camera, scene, and signals; with robot
  sync enabled, physical motion commits only on release.
- **Lanes:** Joint signals share the episode scale and make recording gaps
  explicit instead of compressing them away. One Charming/ECharts canvas owns
  the six aligned grids so their scales, tooltip, and resize lifecycle stay in
  sync; extrema-preserving downsampling bounds the data crossing the Wasm
  bridge. Camera pixels and media state belong exclusively to Camera panels;
  the Timeline does not embed a camera row or thumbnail.
- **Episode items:** Equal 72×36px items favor scanability. Duration and state
  are text/tint, never proportional card geometry.
- **Empty state:** “Record in Live or select an episode in Data.” It names the
  two real ways the panel gains content.

### Signals

Signals is an episode visualization on the Timeline clock, not an unowned
dashboard. Traces share one time axis and degree scale across arms. The readout
prints every plotted value and doubles as the legend. Joint colours are stable
by joint name, and crosshair/focus changes emphasis without changing layout.

### Training

Training is a curve-first run desk, not a collection of dashboard cards. A
30% left rail separates **Training control** above **Training history**. The
remaining workspace is one tab group: every opened run owns one tab containing
its overview, dominant learning curve, data path, throughput, events, and logs.
Opening history never replaces or rewrites the new-run draft.

- **Control and ledger:** Training control selects an available dataset,
  policy, device, optimizer settings, and the collapsed data/tensor query. Its
  single filled **Run training** action stays visible beneath the independently
  scrolling form. An active run adds progress and **Stop after current step**
  in the same control panel. Training history is a separate newest-first ledger
  whose rows state run, dataset, phase, and step progress.
- **Run tabs:** Selecting a history row appends or activates that immutable run
  in the right tab group; already opened runs remain available as sibling tabs.
  Each run tab can be closed without stopping or deleting the backend run.
  Closing the selected tab activates the run to its right, then the one to its
  left; closing the last run restores the empty **Run workspace**.
  Starting a run opens its tab automatically, including when backend state
  arrives after the start response.

- **Shared step axis:** Learning traces, throughput lanes, checkpoints,
  validation, and loader events use optimizer step as one common horizontal
  identity. A marker keeps the same step everywhere so an operator can relate a
  loss change to the system condition that accompanied it.
- **Flat lanes:** Data-path stages and throughput series are content within
  ordinary panels, separated by rhythm and hairlines. They do not introduce
  nested cards, decorative containers, or resting elevation.
- **Charts:** Charming builds typed ECharts options for the linear loss curve,
  its step-axis data zoom, and compact throughput lanes. One local Canvas runtime
  provides the line, grid, tooltip, data-zoom, and Canvas renderer features
  these surfaces use. Authored chart labels, semantic text summaries, and tabular
  readouts remain around the canvas; series colours use centralized roles, and
  colour is never the only legend.
- **Truthful state:** With no run, panels explain how real data appears. After a
  run starts but before its first metric, they say they are waiting for the
  first optimizer step. They never draw synthetic telemetry. Unavailable or
  locked actions remain visible and disabled beside the blocker or active-run
  context, while Stop remains reachable for the run in progress. Readiness is
  the backend's combined remote-compute-and-hosted-Spiral probe, not an
  inference made from settings in the browser. A failed run keeps its terminal
  reason and the latest training log lines visible in its run tab.
- **Owned data path:** The stages shown are the actual boundaries: Rust Spiral
  ADBC query, Arrow image/action buffers, HTTP/Arrow IPC transport, remote
  PyTorch tensor/model compute, and Rust checkpoint ingest. Tensor and
  optimizer throughput are measured values; the Rust loader validates native
  Arrow tensor structs and streams fixed-shape float32 buffers to the selected
  compute runtime.
- **Terse operating copy:** Panel tabs name Training control, Training history,
  and each run; run content uses only the short evidence headers **Loss**,
  **Data path**, and **Throughput**. Configuration uses field labels, run state
  uses measured facts, and SQL plus tensor mapping stays under one collapsed
  **Data query & tensors** disclosure. Data-path stages pair short noun labels
  with live values instead of architecture prose.
- **Desktop composition:** The split tree keeps control, history, and opened run
  panels simultaneously available; container queries may adapt evidence within
  a user-resized panel without creating a viewport-specific application mode.

**The Shared-Optimizer-Step Rule.** Any training view that plots, marks, or
summarizes run progress keys that evidence to optimizer step so learning and
system behavior can be compared without correlating unrelated axes by eye.

**The Immutable-Run-Tab Rule.** A history selection opens the run as a sibling
tab in the run workspace. It never loads historical configuration into the
editable Training control or closes another run the operator is comparing.
The operator may explicitly close a run tab; that removes only the workspace
view and never changes the run's backend lifecycle.

### Rollout

Rollout is a simplified safety console for evaluating a trained checkpoint at
the physical bench. It stays compact enough that the live 3D scene, one
follower camera, and inference evidence remain the visual authority.

- **Checkpoint:** One picker lists checkpoints produced by completed Training
  runs and reveals only a compact policy, validation, and file-size summary.
- **Fixed follower:** The target is always the one robot configured with the
  follower role. Show its identity, motor connection, calibration, and first
  camera assignment in one compact read-only line; never add target-arm
  selection or a second readiness checklist.
- **Execution:** Expose only action steps and control rate. The fixed-follower
  summary already makes the physical target explicit.
- **Readiness and action:** Runtime availability gets one compact note above
  Run policy. Run is the single filled action and its region bottom-anchors on
  tall workbenches. A failed hardware precondition keeps one specific blocker
  adjacent instead of restating every passing check.
- **Current truth boundary:** Training stores the native ACT tensor namespace,
  model configuration, normalization buffers, joint order, camera order, and
  robot type in one SafeTensors artifact. Rollout state and metrics come only
  from the backend-owned native session. Loading and warm-up precede policy
  mode; a failed contract or safety check never becomes physical motion.
- **Inference evidence:** The low lane exposes Latency, Model, and Misses as
  tabular metrics. Values remain em dashes and the lane says “No measurements
  yet” until backend measurements arrive; never draw synthetic timing traces.
- **Desktop composition:** The workbench begins at 27% beside Scene, follower
  cameras, and the three-metric Inference lane.

**The Fixed-Follower Rule.** Rollout never asks the operator to choose a motion
target. It commands only the configured follower, and backend readiness—not
ambient browser selection—decides whether execution may begin.

### Camera Panels

- **Frame:** Dark 16:9 mat inside light panel chrome, with contained pixels and
  honest letterboxing.
- **Default composition:** Each available camera starts in its own row of the
  right-hand camera column so no feed is hidden behind a tab. Docking or tabbing
  remains an explicit user layout choice.
- **Identity:** Tab title states Camera/Recorded plus role. Episode camera tabs
  derive from recorded streams; Live camera tabs derive from current robot
  assignments.
- **Live frame:** The image begins directly beneath its identifying panel tab.
  Do not repeat “role · Live” in an internal metadata row. Unavailable streams
  keep their actionable error inside the frame.
- **Lifecycle:** One stable MJPEG image element per assigned Live camera. Moving
  or tabbing a panel must not fabricate a second connection. Recorded video
  seeks to the playhead and exposes approximate synchronization as caution.
- **Failure:** Actionable unavailable/permission text appears in the frame; a
  green dot never masks a blank image.

### Data, Episodes, and Query

- **Hierarchy:** Dataset owns Episodes. The Episodes header imports or exports
  only the selected dataset's episode-owned relations; episode-level actions
  are Select, Replay, and Delete. Whole-project transfer belongs to Settings →
  SpiralDB.
- **One storage layer:** Episodes remain one newest-first SpiralDB ledger. No
  location grouping, publication badge, Push action, retry queue, or local
  fallback appears because there is no second durable store to reconcile.
- **Episode transfer:** Data exports a versioned `.spiral-episodes` file for the
  selected dataset. Import merges into the selected target dataset and refuses
  episode-ID collisions rather than replacing an existing episode.
- **Catalog refresh:** Episodes load once when the browser starts. Known
  recording, deletion, and import results update the browser-owned catalog;
  one compact Refresh action in the Episodes header explicitly reloads the
  catalog and catalog issues in one request. None uses
  polling or the live event stream.
- **Form decision:** This is an incumbent-surface extension, exempt from a
  concept roll because the operator explicitly retained the existing ordered
  episode list and rejected location grouping or a new transfer composition.
- **Query:** A guarded read-only SQL editor with presets and a row limit in the
  Data workbench. Results render as the active sibling tab beside the preserved
  Scene and may synchronize the selected episode/playhead.
- **Destination state:** Settings selects the Spiral API endpoint, SpFS
  endpoint, and project; first launch opens this section when no project is
  selected. The backend remembers only those bootstrap pointers in its private
  JSON file. A project switch requires an idle system with no visible connected
  robot and replaces the shared repository connection pool atomically. If the
  old catalog is unavailable, released hardware connections hidden by that
  failure are detached as part of the cutover. Endpoint and credential changes
  are scoped to the new database and apply live after the old pool drains;
  Studio does not write project preferences to the still-active old
  destination. Namespace remains process configuration, and Settings offers no
  second storage choice or redundant enable switch. Domain operations and saved
  training runs do not carry alternate destination objects.

### Settings

Settings is one full-height utility panel inside the Studio shell: category
navigation at left, ordinary forms at right, header and footer fixed. It
replaces the current activity workspace while leaving the activity rail and
status bar available. Robots is the default section, followed by SpiralDB,
Training, and Video. SpiralDB owns complete-project backup and destructive restore as a
distinct `.spiral-project` transfer; restore alone uses the shared blocking
dialog, requires explicit confirmation, and replaces all Studio-managed
relations, including the Workflow feature registry and every cache relation
whose source-derived identity that registry validates. Save failures, test results, and configuration blockers remain
visible beside the responsible section and its retry and exit actions.

### Inputs / Fields

- **Style:** 24px minimum height, paper fill, graphite ink, 4px radius, one-pixel
  hairline, and 6px horizontal padding.
- **Focus:** Border and focus ring shift to role blue.
- **Paired action:** A picker and the action that completes it share one row
  when space allows.
- **Joint position:** Writable slider only in Hold/Manual. Release keeps normal
  high-contrast telemetry instead of washed-out disabled sliders.

### WebGL Bench

The stage uses the pale bench clear colour, low-contrast lighting, a sparse
scale grid, subdued axes, and arm identity accents. Camera geometry may be
selectable in the scene, but camera pixels remain in camera panels. Corner
controls are compact and use the same paper/graphite vocabulary as the shell.
Recorded playback samples the episode at a 30Hz visual cadence, and every
accepted recorded-source update is an explicit canvas redraw dependency.

### Notices

Transient notices float over the shell, carry their own 4–14 second expiry, and
keep Dismiss. Failures use alert semantics and a longer reading floor. A notice
never inserts a persistent banner above the workspace.

### Named Rules

**The Focusable-Reveal Rule.** A hover-revealed control fades with opacity and
also reveals on focus. It is never removed with `display` or `visibility` when
it is the only focusable action in its row.

**The Expiring-Notice Rule.** A notice cannot be raised without a deadline; the
deadline belongs to the notice value, not the component that draws it.

**The One-Implementation Rule.** Cross-surface icons, modal geometry, focus
movement, pending-request guards, and value formatting each have one shared
owner. Product components own domain behavior and placement; they do not wrap
another component only to rename it or copy shared mechanics.

## Do's and Don'ts

### Do:

- **Do** keep shell, panels, WebGL, plots, Timeline, settings, and data tools in
  one light neutral world; reserve the dark mat for camera imagery.
- **Do** register independent surfaces as panels and give them durable IDs and
  meaningful home tiles.
- **Do** preserve the one-third/two-thirds Live/Data default and Rollout's 27%
  workbench default while treating persisted user layout as the active
  composition.
- **Do** give every drag, dock, split, resize, and tab action a keyboard path,
  visible focus, and reduced-motion behavior.
- **Do** derive Live camera panels from robot assignments and Data camera panels
  from episode media streams.
- **Do** keep one stable camera node per Live feed and preserve panel identity
  while the layout changes.
- **Do** keep Timeline synchronized with scene, camera, signals, query selection,
  replay, and optional physical seek.
- **Do** print every plotted value and keep joint colours stable by name.
- **Do** keep the base at 12px with 24px controls and a 2/4/6/8px rhythm.
- **Do** use tabular figures and fixed boxes for changing telemetry.
- **Do** render exceptional status only when it helps the operator act.
- **Do** put disabled-action reasons and physical consequences on the page.
- **Do** move robot and destination configuration into Settings.
- **Do** keep Live, Data, Workflow, Training, and Rollout layout state independent, and
  keep episode selection and camera ownership activity-scoped so switching
  cannot move content between them.
- **Do** make Workflow relation-first: multiple Spiral Table roots may converge
  through Query and Model nodes; each Model consumes a visible SQL candidate
  cohort and owns a visible source-derived Studio feature cache.
- **Do** keep Workflow truth layered: the Assistant may inspect, propose, save,
  run, wait, and iterate through bounded backend tools, while the canvas, run
  snapshot, and Results remain authoritative and the canvas keeps manual Run.
  Keep provider readiness, active/waiting/interrupted turn state, and compact
  tool outcomes explicit. Keep the single executable SigLIP capability, actual
  Loading/device/fallback state, bounded progress, quarantine/warnings, and
  computed/reused materialization evidence explicit. Empty Results must
  distinguish no run from real empty rows.
- **Do** route workbench, Timeline, panel-focus, and backend events through one
  activity controller that derives the complete visible projection.
- **Do** represent Live, Recorded Loading, Recorded Ready, Recorded Unavailable,
  and Recorded Failed as explicit visualization-source states.
- **Do** keep one Timeline action vocabulary in every activity and include the
  recording or episode identity in every time or transport action.
- **Do** keep Training curve-first and align learning, resources, and run events
  to the shared optimizer-step axis.
- **Do** author Training and Timeline plots through the shared Charming
  renderer with bounded extrema-preserving payloads, semantic text readouts,
  and centralized series roles.
- **Do** show truthful Training empty, waiting, unavailable, and locked states;
  never substitute synthetic telemetry for backend evidence.
- **Do** keep Rollout fixed to the configured follower, with one live panel per
  recorded follower camera and a specific disabled-action reason beside Run policy.
- **Do** keep Rollout metrics empty until native measurements arrive, keep
  loading/stopping/error states authoritative, and name the exact recovery for
  every blocked or stopped session.
- **Do** keep `tailwind_input` and `tailwind_output` in `Dioxus.toml`; DX owns
  stylesheet compilation for serve, build, and bundle.
- **Do** remove a component, property, selector, token, or dependency feature
  when its last real consumer disappears.

### Don't:

- **Don't** restore the retired fixed Robots / Scene / Properties three-pane
  shell or the full-width dataset rail.
- **Don't** add fixed one-off panel arrangements in the Studio; extend
  `spiral-robotics-panels` when the mechanic is generic and register domain content in
  the Studio.
- **Don't** name product components or local CSS classes after native
  presentation such as `Button`; name the action's domain intent or the layout
  responsibility.
- **Don't** turn every tool into primary navigation. Live, Data, Workflow,
  Training, and Rollout are activities; panel tabs are spatial grouping inside
  them.
- **Don't** put Hugging Face, MCAP, URLs, repository selectors, or import
  controls on the Workflow canvas. Import into Spiral first, then read the
  resulting Table relations.
- **Don't** present Hugging Face model-artifact resolution as dataset ingestion,
  advertise unimplemented model variants, imply artifact/network availability,
  or turn bounded row counts into a fabricated percentage or ETA.
- **Don't** add free-form Workflow edge drawing or generic node deletion to the
  Workflow canvas. Preserve the controlled Add model / Remove model lineage
  transformation and its candidate-query, typed-feature, and shared-cache
  ownership.
- **Don't** put provider keys, provider/model identity, configuration instructions,
  raw provider payloads, media/tensors, or a second
  manual Run control in the Assistant. Do not let chat prose override the saved
  graph, run snapshot, or Results.
- **Don't** let a Data action mutate Live camera tabs, or derive recorded camera
  tabs from current hardware assignment.
- **Don't** duplicate a camera stream merely because its panel moved or another
  panel became visible.
- **Don't** put robot setup back into the active Live flow.
- **Don't** add headings or helper labels that restate the visible region below
  them (“Operate and capture,” “Record always uses Teleop,” “Select an arm”).
- **Don't** create a second Timeline, change its ownership between Query and
  Episodes, or restart replay from zero after Pause.
- **Don't** let scrub move only the playhead; camera, 3D, signals, and video must
  show the corresponding time.
- **Don't** let a workbench mutate visualization panels independently; derive
  primary content, cameras, signals, and Timeline from one activity projection.
- **Don't** treat missing recorded data as permission to show live telemetry or
  a live camera inside a recorded episode context.
- **Don't** infer an action target from ambient selection or by parsing a panel
  ID; carry the episode, stream, robot, or query identity in the action.
- **Don't** animate split geometry while the pointer is dragging or keep panel
  entry animation under reduced motion.
- **Don't** introduce viewport breakpoints or alternate mobile/touch UI.
- **Don't** use colour alone for status, traces, or docking intent.
- **Don't** unfold live prose, encode episode quality in card size, or introduce
  diagnostic dashboards as the primary Data experience.
- **Don't** put camera pixels inside the 3D scene; the scene owns camera
  geometry and selection, camera panels own frames.
- **Don't** write hue-named or one-off colour literals in components; use role
  tokens.
- **Don't** turn Training data-path stages or resource lanes into elevated
  dashboard cards, or give related training evidence unrelated axes.
- **Don't** turn Rollout into an ML dashboard with a target-arm picker,
  model-contract table, sampling controls, or a stack of diagnostic panels.
- **Don't** fabricate rollout motion, readiness, latency, chunk timing, or
  deadline-miss values; every one comes from the backend session.
- **Don't** add uppercase tracked kickers, decorative display type, pill-shaped
  containers, a fixed-dark workspace, or ornamental looping animation.
