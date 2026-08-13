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
  good: "#448361"
  good-soft: "rgb(68 131 97 / 0.1)"
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
  target-touch: "44px"
components:
  button:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  button-danger:
    backgroundColor: "transparent"
    textColor: "{colors.danger}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "{spacing.target}"
  button-link:
    backgroundColor: "transparent"
    textColor: "{colors.ink-soft}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "0"
    height: "{spacing.target}"
  icon-button:
    backgroundColor: "transparent"
    textColor: "{colors.ink-soft}"
    rounded: "{rounded.sm}"
    size: "{spacing.target}"
  icon-button-pressed:
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
  record-button:
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

The Studio has four primary activities: **Live** for operating hardware and
recording, **Data** for episodes and query, **Training** for configuring and
observing policy runs, and **Rollout** for staging a trained checkpoint against
the configured follower. Everything else is a panel. A
reusable tab-and-tile system lets the operator attach a panel as a tab, split a
tile, resize the result, or move it with the keyboard. This borrows VS Code’s
learned spatial grammar without importing its visual weight: the layout is
powerful, but the chrome remains Notion-like, compact, and quiet.

The **Timeline is the shared clock**, not a footer or a dataset rail. It is
registered in both activities and synchronizes recording, replay, camera video,
historical 3D pose, signals, query selection, and optionally a safety-gated
physical seek. Signals belongs to the episode on that clock. Full camera images
belong in camera panels; robot ownership remains explicit through arm and role,
and actionable camera failures stay inside the affected frame rather than being
duplicated in the Live workbench.

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
- Live, Data, Training, and Rollout are the primary activities; independent
  content registers as panels instead of growing one-off shell regions.
- A persisted tab-and-tile workspace supports docking, tabbing, splitting,
  resizing, keyboard operation, and hot-plugged panel reconciliation.
- The default Live/Data desktop composition is one-third workbench and
  two-thirds visualization, with Timeline in the lower 42% and each camera
  visible in its own row of the named camera column. Rollout narrows the
  workbench to 27% and replaces the Timeline slot with one low Inference lane.
- One playhead synchronizes every representation of an episode.
- Compact 12px type, 24px controls, 32px panel tabs, small radii, and hairlines.
- Monospace tabular figures for elapsed time, rates, durations, counts, and IDs.
- Live values use fixed boxes and exceptional status, so telemetry changes no
  surrounding geometry.
- Role blue marks active, selected, primary, and docking-preview states;
  semantic colour is reserved for real state.
- Narrow screens replace the desktop tree with one horizontally selectable
  panel at a time without discarding the saved desktop layout.

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
  readout cells, and mobile switcher troughs.
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
- **Body** (400, 12px): Buttons, fields, ordinary values, and sentences.
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

- a 60px activity rail spanning the body, with Live/Data/Training/Rollout at the
  top and Settings anchored at the bottom;
- the remaining body area owned by one `PanelWorkspace`, without a separate top
  menu row;
- a 20px bottom status bar for connection, environment, activity, recording,
  torque, rate, and issues;
- transient notices floating above the shell rather than inserting a new row.

Live, Data, Training, and Rollout are activity changes, not panel tabs. Each
activity registers its own panel set and uses an independent versioned
persistence key. Switching activity therefore changes both the operating
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
ratios so neither side collapses. Split buttons may create an explicit empty
tile as a truthful drop target.

Panel identity is durable and unique. A panel cannot appear in two tiles.
Camera panels reconcile by named home when cameras are assigned, removed, or
loaded asynchronously. Content remains keyed by panel ID so moving or tabbing a
camera does not restart an unrelated feed. Layout mechanics, persistence,
keyboard behavior, previews, and chrome belong to `spiral-robotics-panels`; the Studio
owns panel titles, content, safety, and domain state.

The Timeline is always registered in both activities, including empty state.
Its default lower position gives the system a consistent clock, but the user may
move it like any other panel. Live review and Data selection are separate state:
recording updates Live’s inspectable take, while Data owns its selected episode
and query result.

Live begins with Control; recording identity, status, and capture transport stay
in the Timeline and global status bar. Robot identity appears once in a compact
switcher. There is no selected-arm toolbar beneath it. Setup is global Settings
→ Robots rather than a collapsible section in Live.

Data states `Dataset → Episodes` explicitly. Episodes owns select, replay, and
delete. Query is a peer tool within Data; its result joins the selected
episode's Scene as a sibling tab and never unregisters that Scene. The Timeline
keeps the same layout contract. Recorded camera panels come from the selected
episode’s media streams, not the currently connected camera assignments.

Training is curve-first: its first-open composition gives the learning curve
the dominant visualization area, keeps the literal data path beside it, and
places resource lanes below. The run builder remains a peer panel at left, and
the whole activity persists through the same tab-and-tile workspace mechanics
as Live and Data.

Rollout is a compact physical-policy bench, not an ML dashboard. Its first-open
desktop composition uses a 27% workbench beside a dominant live Scene, the
first assigned follower camera, and one low Inference evidence lane. The
workbench contains only checkpoint selection, fixed-follower readiness, simple
execution inputs, and the primary Run policy action; on a tall workbench, that
action area anchors to the bottom. Follower connection, calibration, and camera
state read as one compact line; do not rebuild them as a readiness checklist.

At 760px and below, the split tree is not squeezed into unusable slivers. A
horizontally scrollable panel switcher presents one registered panel at a time;
the desktop tree remains persisted and unchanged. At narrower widths, optional
status facts may drop, but active recording, torque, blockers, and the control
that stops motion remain reachable.

At 480px and below, Rollout uses that same mobile panel switcher. Its execution
fields stack into one column, the three essential inference metrics remain one
compact strip, and interactive controls preserve 44px reachability.

### Named Rules

**The Panel-Owns-Content Rule.** If a surface has independent identity, focus,
or sizing needs, register it as a panel. Do not add another fixed shell region
or let one component mutate another tile’s tabs.

**The Timeline-Is-One-Clock Rule.** Live, Episodes, Query, camera media, scene,
signals, and replay share one playhead contract. Do not create a second timeline
with different geometry or ownership.

**The Nothing-Load-Bearing-Hidden Rule.** A breakpoint may reflow, switch,
scroll, or reduce optional status. It may not hide an active recording, safety
state, blocker, or stop/release control.

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
│   ├── ActivityRail                         Live / Data selection; Settings access
│   └── PanelWorkspace                       layout mechanics only
│       ├── SemanticWorkbenchPanel
│       │   ├── LiveWorkbench                operate / record semantics
│       │   ├── DataWorkbench                episode / query semantics
│       │   ├── TrainingWorkbench            train / observe semantics
│       │   └── RolloutWorkbench             fixed-follower policy semantics
│       ├── PrimaryVisualizationPanel
│       │   ├── Scene
│       │   ├── QueryResult
│       │   ├── TrainingEvidence
│       │   └── EmptyOrLoadingState
│       ├── CameraPanel(s)
│       └── EvidencePanel
│           ├── TimelinePanel                one shared episode-clock contract
│           └── RolloutInferencePanel        measured policy-loop evidence
├── StatusBar
└── OverlayLayer                             settings / notices / confirmations
```

`StudioShell` owns the application frame. `ActivityRail` chooses the active
activity and owns the bottom-anchored Settings entry. `PanelWorkspace` owns
tabs, docking, splits, resizing, mobile panel switching, reconciliation, and
persistence. It does not know what an episode, camera, robot, query, or
playhead means.

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

Rollout's Scene and camera are explicit live sources scoped to the configured
follower: the Scene may show both arms for physical context, while the one
camera panel comes from the follower's first assigned camera. Inference
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
- **Segment lift** (`0 1px 3px rgb(0 0 0 / 0.12)`): Active mobile panel tile.
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

### Buttons

- **Shape:** Compact rectangle, 4px radius, 24px minimum height, and 6px
  horizontal padding. Record is the single 28px exception.
- **Primary:** Role-blue fill with white text, once per local purpose: Record,
  Replay, Run query, Connect, or Save.
- **Danger:** Red text on transparent with a soft wash on hover. Delete and
  Release consequences confirm inline rather than becoming a filled red CTA.
- **Link:** Quiet 11px text action for Rescan, Identify, Export, Delete, and
  Dismiss.
- **Focus:** The global accent focus ring remains visible inside dense chrome.
  Disabled consequential actions show a reason beside them.

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
  surface. Live, Data, Training, and Rollout stay at the top; Settings stays at
  the bottom. Active activities use blue ink, a paper surface, and a 2px left
  edge. There is no separate top menu.
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
- **Charts:** Charming builds typed ECharts options for the logarithmic loss
  curve and compact throughput lanes. One local tree-shaken Canvas runtime
  registers only the line, grid, tooltip, and Canvas renderer features these
  surfaces use. Authored chart labels, semantic text summaries, and tabular
  readouts remain around the canvas; series colours use centralized roles, and
  colour is never the only legend.
- **Truthful state:** With no run, panels explain how real data appears. After a
  run starts but before its first metric, they say they are waiting for the
  first optimizer step. They never draw synthetic telemetry. Unavailable or
  locked actions remain visible and disabled beside the blocker or active-run
  context, while Stop remains reachable for the run in progress. Readiness is
  the backend's combined Burn-runtime-and-hosted-Spiral probe, not an inference made
  from settings in the browser. A failed run keeps its terminal reason and the
  latest training log lines visible in its run tab.
- **Owned data path:** The four stages shown are the actual boundaries: Rust
  Spiral ADBC query, Arrow image/action buffers, Burn tensor/model compute, and
  Rust checkpoint ingest. Tensor and optimizer throughput are measured values;
  the Rust loader validates native Arrow tensor structs and packs fixed-shape
  float32 buffers directly for Burn.
- **Terse operating copy:** Panel tabs name Training control, Training history,
  and each run; run content uses only the short evidence headers **Loss**,
  **Data path**, and **Throughput**. Configuration uses field labels, run state
  uses measured facts, and SQL plus tensor mapping stays under one collapsed
  **Data query & tensors** disclosure. Data-path stages pair short noun labels
  with live values instead of architecture prose.
- **Responsive:** At 760px and below, the shared panel switcher exposes control,
  history, and opened runs one at a time. The selected run stacks loss, data
  path, and throughput without hiding the tab history or consequential actions.

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
- **Current truth boundary:** Training publishes the native ACT tensor namespace,
  model configuration, normalization buffers, joint order, camera order, and
  robot type in one SafeTensors artifact. Rollout state and metrics come only
  from the backend-owned native session. Loading and warm-up precede policy
  mode; a failed contract or safety check never becomes physical motion.
- **Inference evidence:** The low lane exposes Latency, Model, and Misses as
  tabular metrics. Values remain em dashes and the lane says “No measurements
  yet” until backend measurements arrive; never draw synthetic timing traces.
- **Responsive:** The desktop workbench begins at 27%. At 480px and below,
  Rollout uses the shared mobile panel switcher, stacks fields, keeps the three
  metrics in one compact strip, and preserves 44px interactive controls.

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

- **Hierarchy:** Dataset owns Episodes. Dataset-level actions include Import and
  Export; episode-level actions are Select, Replay, and Delete.
- **Local staging:** Episodes remain one newest-first ledger; storage location
  never creates a second grouping or navigation mode. Every row states its
  SpiralDB status as Local, Queued, Pushing, Synced, or Failed beside the
  recording state.
- **Push semantics:** Local rows expose Push, failed SpiralDB attempts expose
  Retry, and Synced rows need no redundant action. Push/Retry targets only the
  configured SpiralDB destination. Removing a Synced row deletes only its local
  copy and leaves the persisted SpiralDB episode intact.
- **Form decision:** This is an incumbent-surface extension, exempt from a
  concept roll because the operator explicitly retained the existing ordered
  episode list and rejected location grouping or a new transfer composition.
- **Query:** A guarded read-only SQL editor with presets and a row limit in the
  Data workbench. Results render as the active sibling tab beside the preserved
  Scene and may synchronize the selected episode/playhead.
- **Destination state:** SpiralDB becomes active when its required configuration
  is complete; do not add a redundant enable switch.

### Settings

Settings is one Codex-style modal: category navigation at left, ordinary forms
at right, header and footer fixed. Robots is the default section, followed by
SpiralDB and Video. Test results and configuration blockers are rendered beside
the responsible section.

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
- **Do** keep Live, Data, Training, and Rollout layout state independent, and
  keep episode selection and camera ownership activity-scoped so switching
  cannot move content between them.
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
- **Do** keep Rollout fixed to the configured follower, with one live follower
  camera and a specific disabled-action reason beside Run policy.
- **Do** keep Rollout metrics empty until native measurements arrive, keep
  loading/stopping/error states authoritative, and name the exact recovery for
  every blocked or stopped session.
- **Do** keep `tailwind_input` and `tailwind_output` in `Dioxus.toml`; DX owns
  stylesheet compilation for serve, build, and bundle.

### Don't:

- **Don't** restore the retired fixed Robots / Scene / Properties three-pane
  shell or the full-width dataset rail.
- **Don't** add fixed one-off panel arrangements in the Studio; extend
  `spiral-robotics-panels` when the mechanic is generic and register domain content in
  the Studio.
- **Don't** turn every tool into primary navigation. Live, Data, Training, and
  Rollout are activities; panel tabs are spatial grouping inside them.
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
- **Don't** hide a consequential action, blocker, active recording, or torque
  state at a breakpoint.
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
