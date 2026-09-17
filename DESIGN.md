---
name: Spiral Robotics Studio
description: A compact, dockable one-light workbench for operating robots, understanding episodes, curating them into kept sets, training policies, and staging fixed-follower rollouts.
colors:
  canvas: "#ffffff"
  surface: "#ffffff"
  sunken: "#f7f7f5"
  viewport: "#f5f5f3"
  camera: "#1f1f1f"
  lift: "rgb(0 0 0 / 0.12)"
  hover: "rgb(55 53 47 / 0.06)"
  line: "#e8e8e6"
  line-strong: "#cfcfcb"
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
  stage-source: "#5b7fb8"
  stage-media: "#b5701f"
  stage-model: "#8b5cc7"
  stage-shape: "#65645f"
  stage-join: "#3d8f86"
  stage-output: "#2f7d5b"
  category-1: "#2a78d6"
  category-2: "#eb6834"
  category-3: "#1baf7a"
  category-4: "#c98f00"
  category-5: "#d9679a"
  category-6: "#7560b8"
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
  action-square:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    rounded: "{rounded.sm}"
    padding: "0"
    size: "{spacing.target}"
  document-bar:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.title}"
    padding: "0 8px 0 12px"
    height: "36px"
  stage-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.title}"
    rounded: "{rounded.sm}"
    padding: "10px 12px 8px"
    width: "224px"
    height: "92px"
  stage-card-selected:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
  stage-mark:
    backgroundColor: "transparent"
    textColor: "{colors.stage-source}"
    rounded: "{rounded.sm}"
    size: "18px"
  stage-strip-chip:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.micro}"
    rounded: "{rounded.sm}"
    padding: "0 6px"
    height: "22px"
  run-strip:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.label}"
    padding: "4px 12px"
    height: "32px"
  result-category-chip:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "1px 6px"
  result-image-preview:
    backgroundColor: "{colors.camera}"
    rounded: "3px"
    width: "96px"
    height: "54px"
  side-sheet:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    width: "320px"
  side-sheet-heading:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    padding: "0 6px 0 12px"
    height: "36px"
  row-inspector:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    padding: "12px"
    width: "320px"
  assistant-request:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "8px 12px"
  composer-field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "8px 40px 8px 12px"
  curation-tile:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "6px 8px"
  curation-tile-struck:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink-faint}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "6px 8px"
  curation-tile-selected:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
  curation-reason-chip:
    backgroundColor: "color-mix(in srgb, var(--hue) 13%, transparent)"
    textColor: "color-mix(in srgb, var(--hue) 55%, {colors.ink})"
    rounded: "{rounded.sm}"
    padding: "1px 6px"
  curation-rule-mark:
    backgroundColor: "transparent"
    rounded: "{rounded.full}"
    size: "16px"
  curation-rule-dot:
    backgroundColor: "{colors.category-2}"
    rounded: "{rounded.full}"
    size: "6px"
  curation-frame:
    backgroundColor: "{colors.camera}"
    rounded: "{rounded.sm}"
    width: "100%"
---

# Design System: Spiral Robotics Studio

## Overview

**Creative North Star: "The One-Light Control Bench"**

Spiral Robotics Studio is one calm, literal operating workspace. Warm-white panels,
graphite type, pale WebGL, hairline structure, small radii, and tabular data
make it feel like a precise bench utility rather than a dashboard. Camera pixels
alone keep a dark mat for honest letterboxing; application chrome never changes
visual worlds when the operator changes tasks.

The Studio has six primary activities: **Live** for operating hardware and
recording, **Data** for catalog inspection, import, and query, **Workflow** for deriving and
querying Spiral relations, **Curation** for judging every episode of a dataset
against a written recipe, **Training** for configuring and observing policy
runs, and **Rollout** for staging a trained checkpoint against the configured
follower. Everything else is a panel. A
reusable tab-and-tile system lets the operator attach a panel as a tab, split a
tile, resize the result, or move it with the keyboard. This borrows VS Code’s
learned spatial grammar without importing its visual weight: the layout is
powerful, but the chrome remains Notion-like, compact, and quiet.

The **Timeline is the shared clock**, not a footer or a dataset rail. It is
registered in Live and synchronizes recording, replay, camera
video, recorded 3D pose, signals, query selection, and optionally a
safety-gated physical seek. Signals belongs to the episode on that clock. Full
camera images belong in camera panels; robot ownership remains explicit through
arm and role, and actionable camera failures stay inside the affected frame
rather than being duplicated in the Live workbench.

Density is the point, not a side effect. The base face is 12px, ordinary
controls are 24px tall, and most gaps come from a 2/4/6/8px rhythm. Record is
the one 28px control because it may be reached with a hand still on the arm.
The 16px elapsed readout emphasizes capture time. Colour is spent on
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
- Live, Data, Workflow, Curation, Training, and Rollout are the primary
  activities; independent content registers as panels instead of growing
  one-off shell regions.
- A session-owned tab-and-tile workspace supports docking, tabbing, splitting,
  resizing, keyboard operation, and hot-plugged panel reconciliation.
- The default Live desktop composition is one-third workbench and
  two-thirds visualization, with Timeline in the lower 42% and each camera
  visible in its own row of the named camera column. Rollout narrows the
  workbench to 27% and replaces the Timeline slot with one low Inference lane.
- Data defaults to a 21% Datasets rail beside an episode workspace: Plots,
  Messages, and SQL as tabs in the primary tile, one panel per recorded
  camera in a right-hand column, and a full-width Timeline across the lower
  40%. The Timeline lists every recorded stream with its message density and
  is where streams are shown, plotted, and selected. Import opens as a peer
  tab in the primary tile.
- Workflow puts the graph first: a 24% Assistant/SQL column at the left, then
  the Graph (74%) beside Stage details (26%) over a Results shelf in the lower
  54%. It has its own layout scope, and every tile stays a dockable workbench
  panel.
- Workflow's data is vivid because its cells know what they are: image
  previews lead, repeated decisions become tinted chips, measurements carry a
  bar between the column's extremes, timestamps read as UTC clock time, and
  identifiers step back into mono. Stage cards wear one of six kind glyphs and
  identity tints; a run animates its edges and shows skeleton rows until Spiral
  answers. Every inference is display-only and the stored value stays exact in
  tooltips and the row inspector.
- Curation is a contact sheet: a 26% Recipe column at the left and the
  Sheet, a run strip over the tile grid, at the right. Every episode is a
  tile; kept tiles stay in full ink on paper, struck tiles recede onto the
  recess and carry the words of the rule that removed them in that rule's
  own tint. The selected episode's evidence docks inside the Sheet as the
  shared side sheet rather than holding a third panel. Workflow and Curation
  share one document bar, state chip, switcher, and run strip.
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
- **Stage kinds** (`{colors.stage-source}`, `{colors.stage-media}`,
  `{colors.stage-model}`, `{colors.stage-shape}`, `{colors.stage-join}`,
  `{colors.stage-output}`): What a workflow stage does to the data. Each paints
  only an 18px glyph tile and its 14% wash, the one-word kind in the card
  footer, and the 14px tile in the overview, so the hue is never the only
  carrier. Model lends its tint to the Assistant's conversation
  tile and to function names in syntax colour; Media lends its amber, mixed
  toward ink, to literals.
- **Result categories** (`{colors.category-1}` through `{colors.category-6}`):
  Washes for repeated string values in a result table, assigned by a value's
  rank within that result. The wash is the hue at 13%; chip text mixes the hue
  55% toward ink so it clears 4.5:1 on its own wash. The same six hues never
  encode health, and a chip's text is always the stored value itself. Curation
  deals the same six washes to exclusion rules in recipe order as
  `category-2, 4, 3, 5, 6, 1`, so the first rules sit far from role blue and
  a lit tile ring is never mistaken for the selected tile's accent ring. A
  rule's tint paints its 6px mark in the recipe, the reason chip on every
  tile it struck (same 13% wash / 55% ink-mix recipe), a struck tile's
  hairline mixed 45% toward the hairline, and the inset ring of the hover
  sweep.
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
- **Strong Hairline** (`{colors.line-strong}`): Connectors in the stage
  strip, resting tool-step dots, and the hover edge of outlined chips: a rule
  that must read against the recess or beside 10px type.
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
`--tone-*` properties in `crates/frontend/tailwind.css`, including
semantic training-series roles. The panel crate maps its `--rp-*` roles to
those tokens. Renderer-owned arm and signal colours remain centralized with
the code that paints them; components do not restate their literals.

**The Blue-Comes-Last Rule.** Identity tints dealt from the category washes
start away from role blue and reach it last, because the accent already
means selection. A tint that could be confused with the selection ring is
never the first one dealt.

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
  Live/Data/Workflow/Curation/Training/Rollout at the top and Settings
  anchored at the bottom;
- the remaining body area owned by one `PanelWorkspace`, without a separate top
  menu row;
- a 20px bottom status bar for connection, environment, activity, recording,
  torque, rate, and issues;
- transient notices floating above the shell rather than inserting a new row.

Live, Data, Workflow, Curation, Training, and Rollout are activity changes,
not panel tabs. Each activity registers its own panel set and uses an independent session
layout scope. Switching activity therefore changes both the operating
content and its visualization without moving Live camera tabs into Data, or
vice versa.

The canonical first-open Live desktop layout is:

```text
┌──────────── one third ────────────┬────────────── two thirds ──────────────┐
│ Live workbench                    │ Scene + Query tab  │ Camera tabs       │
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
keyboard behavior, previews, and chrome belong to the external
[`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench) crate; the Studio owns
panel titles, content, safety, and domain state.

The Timeline is registered in Live, including empty state. Its default lower
position gives capture a consistent clock, but the user may move it like any
other panel. A paused recording remains inspectable in Live; completed native
datasets and their episode outputs are browsable in Data.

Live begins with Control; recording identity, status, and capture transport stay
in the Timeline and global status bar. Robot identity appears once in a compact
switcher. There is no selected-arm toolbar beneath it. Setup is global Settings
→ Robots rather than a collapsible section in Live.

Data follows the operator's own model of their recordings: a dataset is a
folder of MCAP files and each file is one episode. The Datasets rail lists
datasets, expands each into its chronological episodes (labelled by the part
of the source path that differs between files), and folds ordinary tables
and views into a quiet Tables & views section that opens
SQL. Selecting a dataset shows its Overview; opening an episode replaces it
with Plots and Messages tabs, one panel per recorded camera, and the Timeline.
The canonical first-open Data layout is:

```text
┌── 21% ──┬────────── 64% ──────────┬──── 36% ────┐
│ Datasets│ Plots │ Messages │ SQL   │ camera rows │
│ rail    │                         │             │
│         ├─────────────────────────┴─────────────┤
│         │ Timeline — transport, ruler, one row  │
│         │ per stream with message density       │
└─────────┴───────────────────────────────────────┘
```

The Timeline is the navigation hub of an episode: ticking a stream shows it
(a camera opens its panel; a data stream draws every numeric field in Plots),
expanding a data stream exposes its fields for one-by-one selection, and
clicking a stream name selects it for Messages. Cameras, Plots, the ruler,
and the stream rows share one playhead and one visible span.

Curation is a contact sheet. Under the same 36px document bar Workflow uses
(glyph · name ▾ · state chip · Discard Save ▶Run), the `curation-sheet-v2`
layout scope opens as two tiles: a 26% Recipe column beside the Sheet. There
is no third panel; the selected episode's evidence docks at the Sheet's
right edge as the shared 320px side sheet while one is open, so nothing sits
empty at rest and the whole dataset keeps the height. Workflow and Curation
share the document bar, state chip, rename form, switcher popover, and run
strip as one `document-*` / `run-*` class family, so the two document
activities keep one chrome in the same place. The canonical first-open
Curation layout is:

```text
┌── 26% ──┬──────────────── 74% ────────────────┐
│ Recipe  │ Sheet — run strip over the tile grid │
│ Source  │                                      │
│ Measure │  tiles …            ┊ Episode N of M │
│ Rules   │                     ┊ (side sheet,   │
│ Transf. │                     ┊  while open)   │
└─────────┴──────────────────────────────────────┘
```

Both Curation tiles are dockable workbench panels with a leading tab icon.
The grid is `repeat(auto-fill, minmax(11rem, 1fr))` with a 6px gap and
12px sheet padding, so the number of columns follows the tile's width rather
than a breakpoint; at 1100px the sheet still shows the whole dataset.

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
│       │   ├── CurationRecipe               recipe / rule semantics
│       │   ├── TrainingWorkbench            train / observe semantics
│       │   └── RolloutWorkbench             fixed-follower policy semantics
│       ├── PrimaryVisualizationPanel
│       │   ├── Scene
│       │   ├── QueryResult
│       │   ├── WorkflowCanvas
│       │   ├── CurationSheet                tiles; hosts EpisodeSheet in a SideSheet
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

`SideSheet` is a shared component, not a panel: the docked inspector that
opens beside a table or grid for the one selected item. It owns the heading,
the previous / next / close controls, the keyboard (Escape closes, ↑ and ↓
step), and the entering motion; the caller owns the body. Data's
`QueryResult` mounts it as the row inspector and `CurationSheet` mounts it
as `EpisodeSheet`, one episode's facts, verdicts, and frames.

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
Fetched backend state ─┘                                  ├── Camera panels
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
| Episode catalog, selected episode with attached full data, Data tool, and query | Data Activity Controller | Data Workbench and the complete Data projection |
| Workflow saved graph, local draft, typed feature definitions, source-derived cache metadata, run snapshot, and terminal result | Workflow Activity Controller, reconciled with backend truth | Node panel, Workflow canvas, Results panel, Assistant, Status Bar |
| Workflow Assistant readiness, bounded project-scoped conversations, active selection/turn, and typed tool summaries/receipts | Backend Workflow agent, reconciled by the Workflow Activity Controller | Assistant panel; canvas and Results consume only authoritative Workflow state |
| Curation saved recipe, local draft, judged run outcome, hovered rule, sheet filter, lens and order, and selected episode | Curation Activity Controller, reconciled with backend truth | Recipe, Sheet (with the docked EpisodeSheet), document bar, Status Bar |
| Rollout checkpoint, execution inputs, fixed follower, and readiness | Rollout Activity Controller, reconciled with backend truth | Rollout Workbench, live Scene, follower Camera, Inference lane |
| Timeline subject, playhead, preview playback, and Follow Live | Timeline Controller within the active activity | Scene, Cameras, Signals, Timeline |
| Panel geometry and active tab | `PanelWorkspace` | Panel chrome and persisted layout only |
| Hardware, recording, replay, storage, and safety state | Backend | Activity Controller through the wire API |

Primary visualization content, camera content, and Timeline content are derived
projection outputs. They are not additional independent sources of semantic
truth.

### Required Interaction Flows

**Select an episode:** `DataWorkbench → SelectEpisode(id) → Data Activity
Controller → selected catalog entry (EpisodeInfo + full data state) → Scene +
Cameras + Signals + Timeline`.

**Open Query:** `DataWorkbench → ShowQuery(query) → Data Activity Controller →
preserved Scene + active QueryResult sibling tab + explicit episode context or
explicit no-episode context`.

**Apply and run a Workflow:** `Node inspector edits → revision-checked Apply →
Workflow backend → optional pinned SigLIP feature-cache backfill → read-only Query →
refetched run/cancel state → Canvas + bounded media-aware Results`,
while browser-owned node geometry and panel focus remain stable. The controlled
Add/Remove model actions rewrite the Table→Query lineage and placeholder as one
topology change; they never expose arbitrary graph editing.

**Ask the Workflow Assistant:** `Composer → backend-owned provider turn →
relation/model inspection → read-only SQL candidate definition → validated
graph save → optional Workflow run → bounded query inspection → durable thread
+ fetched Assistant state`. The selected conversation alone supplies provider context;
server-scoped create/select changes conversation state without changing the
Workflow graph or run. The Assistant lane periodically fetches text and compact tool summaries. When a run exceeds the bounded
wait, the backend publishes its terminal state, appends a synthetic completion,
and resumes the same turn. Saved canvas state, run snapshots, and Results are
authoritative throughout; Assistant activity never creates a browser-local
graph or a second Run control.

**Seek or rewind:** `Timeline → Seek(subject, time, phase) → Activity Controller
→ one synchronized recorded projection`. A committed, explicitly armed
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
  a visualization, such as the view cube, the graph's zoom group, and its
  minimap. Stage cards and stage-strip chips use the same lift one step
  lighter (`0 1px 2px rgb(0 0 0 / 0.04)`), because they sit on the bench, not
  in the chrome.
- **Hover lift** (`0 2px 6px rgb(0 0 0 / 0.06)`): A stage card under the
  pointer, together with a 1px rise. The "Jump to latest" float uses the
  same geometry at 0.08.
- **Selection glow** (`inset 0 0 0 1px {colors.accent}, 0 2px 8px` accent at
  14%): The selected stage card only; the inset ring is the state, the glow
  is the card returning to rest.
- **Focus ring** (`0 0 0 2px` accent at 15%): The Assistant composer, the run
  selector, and the rename field, paired with an accent border.
- **Dock preview lift** (`0 6px 18px rgb(0 0 0 / 0.08)`): The blue docking
  footprint while a tab is dragged.
- **Dock compass lift** (`0 6px 18px rgb(0 0 0 / 0.12)`): The small floating
  five-zone orientation cue.
- **Floating notice** (`0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)`):
  Settings-adjacent popovers and bottom notices only.
- **Inset state** (`inset 0 0 0 1px currentColor`): Selection and disabled
  state, not elevation. The contact sheet uses the same 1px inset ring in
  the accent for the selected tile and in the hovered rule's tint
  (`inset 0 0 0 1px var(--tile-tone)`) for every tile that rule struck.

### Named Rules

**The Flat-by-Default Rule.** Resting panels, tabs, cards, controls, Timeline,
and settings do not cast shadows. The graph bench is a visualization, so the
objects standing on it (stage cards, strip chips, zoom group, minimap) may
carry the resting lift; nothing inside a panel's own chrome does.

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
  padding. Prominent transport actions use 32px. `ui-action--square` makes
  any variant a 24px square with no padding while keeping its fill; it exists
  for the Assistant's filled send and outline stop. `ui-icon--spin` turns the
  shared spinner glyph once every 900ms and is the only self-moving icon.
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

### Side Sheet

The docked inspector shared by Data's row inspector and Curation's episode
evidence (`.side-sheet`, `components/side_sheet.rs`). One component, two
callers; the callers own only the body.

- **Geometry:** 320px wide, at most 45% of its tile, `shrink-0`, a left
  hairline, paper surface, its own vertical scroll. It sits at the right edge
  of the table or grid it inspects and takes width from it; it never overlays.
- **Heading:** Sticky, 36px minimum, hairline below, 12px/6px padding:
  a 12px semibold tabular title ("Row N", "Episode N") with the detail
  ("of M") in regular weight and faint ink, then a `↑ ↓ ×` icon-action group
  at 1px gaps. Focus lands on the title when the sheet opens (2px accent
  outline at 3px offset); the step actions disable at either end.
- **Keyboard:** Escape closes and returns focus to the invoking tile or row;
  ↑ and ↓ step to the neighbour. The same keys are on the actions' tooltips.
- **Motion:** Enters over 180ms on `--ease-out-expo` from 6px right;
  removed under `prefers-reduced-motion`.

### Application Chrome

- **Activity rail:** 60px wide on desktop and the only global navigation
  surface. Live, Data, Workflow, Curation, Training, and Rollout stay at the
  top, in that order; Settings stays at the bottom. Active activities use blue ink, a paper
  surface, and a 2px left edge. There is no separate top menu.
- **Status bar:** 20px, 10px type, hairline-separated facts. It owns global
  connection, environment, current activity, recording, holding, rate, and
  issue truth. In Curation it reads connection · dataset · the run phase only
  while it is news (Running or Failed; hidden when idle or complete) · the
  verdict "N of M kept" in the accent.

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

**Operate.** The graph is how an engineer understands what a workflow does,
so it owns the center; Results prove it beneath, and the Assistant authors it
from a tall column at the side.

- **Composition:** One 36px bar, then a 24/76 horizontal split. The bar
  opens with the workflow glyph and a 22px Build | Compare segmented control
  that stands in the same place in both views, so switching never moves the
  header; Build fills the rest of the bar with the document identity and
  actions, Compare with the pair of workflows and the Compare action. The left
  24% is a tall console tile holding Assistant and SQL as tabs. The right 76%
  divides 46/54 vertically: above, the Graph (74%) beside Stage details (26%);
  below, Results across the full width. A pipeline is wider than it is tall, so
  the graph row is sized for two stacked sources and the results shelf keeps
  about five rows of previews. The `workflow-investigation-v12` layout scope
  applies this arrangement; every tile remains a dockable, resizable workbench
  panel with a leading tab icon.
- **Document bar:** A 36px paper line with a lower hairline, shared with
  Curation as the `document-bar` family (identity, glyph, title, name, state
  chip, actions, rename form, run action, switcher, and backdrop) and the
  `run-strip` family (phase, select, count, duration, elapsed, executed SQL).
  After the glyph and the view switch, the name as a quiet 12px semibold button with a 12px
  chevron that turns when open, and a keyed 11px state chip with a 6px dot
  (Saved · green dot, Unsaved changes · ochre, Changed in another session ·
  red) sit at left; Discard, Save, and the filled Run with a play glyph sit at
  right. Run turns
  into a spinning "Running" while a query is admitted, keeping its 72px
  minimum width so the bar does not shift. The name button opens one popover:
  a 320px paper sheet with a 6px radius and the floating-notice shadow, holding
  a listbox of saved workflows with the current one checked, then New blank
  workflow, From dataset, and Rename behind a hairline. Escape and a click on
  the transparent backdrop close it and return focus to the title. The title
  always opens; when switching is blocked (unsaved changes, an active
  Assistant turn, loading) the reason sits at the top of the popover on the
  caution wash and the list is disabled, so the bar never prints a status
  sentence beside the state chip.
- **Graph:** Reads left to right on the pale bench with a 22px dot grid.
  Stages follow the author's ideas, not the planner's operators, and routine
  SQL is condensed so the native and model work stands out: every linear run
  of filters, projections, windows, sorts, and limits between two meaningful
  stages is one card, titled by the operation that changes the row population
  (consecutive filters state both conditions joined by AND) and footed by the
  folded operations in words ("filter · number · select"); pure column
  selection and ordering fold into whichever stage feeds them (a read, a
  native call, a join); numbering rows inside groups and then keeping row 1
  reads as "One row per …" only when that filter follows the window; every
  SPQL call is its own stage, named by what it does (Score images with its
  prompt, Convert frames to RGB with its profile); a CASE becomes "Classify
  rows" listing its outcomes; joins name their type and keys and label their
  edges left and right in 10px; UNION, INTERSECT, and EXCEPT are Join-kind
  stages ("Combine rows · All rows from both inputs, duplicates kept") whose
  peer inputs carry no labels. Casts render as their input. Stage cards are 224×92 paper with a hairline, 4px
  radius, and the resting lift: an 18px kind glyph tile (4px radius, 12px
  icon, 14% wash) beside a 12px semibold title, a two-line 11px/15px summary
  painted token by token when it is an expression, and a 10px footer naming
  the kind in its tint, then the native function, provider, or folded
  operation count in mono. Six kinds carry identity tints: Source, Media,
  Model, SQL, Join, Output. Hover darkens the hairline toward faint ink and
  lifts 1px; selection is the accent hairline plus inset ring and a 14% accent
  glow. Cards enter once, left to right, 320ms each and 45ms apart on the
  Studio easing; edges (1.25px, faint ink at 72%) fade in 180ms after them.
  While a run is admitted every edge turns accent, thickens to 1.5px, and
  carries 5/5 marching dashes on a 700ms cycle. Handles are 6px, strong-line,
  and inert; they turn accent with the card. Nothing else sits on the canvas:
  the stage count, zoom, and fit share one hairline group with the resting
  lift at bottom left, and the Details overview is the complete list of
  stages, each row selecting and centring its card. Cards never render below
  0.7 zoom: a graph that fits at readable zoom is fitted, and one that does
  not keeps its cards at the largest size the pane's height allows, starts at
  its sources, stays pannable, and shows a small 4px-radius minimap at bottom
  right. After the user moves the viewport, resizing preserves graph-space
  center and zoom.
- **Stage details:** Its own tile beside the graph, so selecting a stage never
  resizes the canvas. Provider names read as what they are ("Video topic",
  "Episodes"), planner-internal window columns are never shown, and a read's
  column list clamps to two lines in the overview. It answers, in order: what the stage does (heading with
  kind tile, 12px semibold title, "Kind · footer" in 10px, the 11px summary,
  and the full classification rule on the recess when there is one); what it
  reads or combines (Left/Right for joins); Details as a label/value grid of
  facts drawn from the native properties (10px faint labels, 11px ink values;
  dataset, topic, provider, revision; join type and keys; conditions; groups
  and order; computed columns as `name = expression`; function, call, and
  produced type); Steps for folded stages as a decimal list with tabular
  markers; Columns produced as an 11px schema table with a count and a green
  "new" mark (12% wash) on every column the stage adds over its inputs, its
  row washed 7% green; then the native operators and Full plan JSON behind
  hairline disclosures. Selecting a connection shows what travels along it.
  Before any selection the tile shows the workflow overview derived from the
  same plan: what it reads (dataset, topic, provider), what it combines (join
  type and keys), how many columns it returns, and every stage as a 32px
  clickable row (glyph tile, 11px title, 10px summary) that selects and
  centres it on the graph. The body settles in over 200ms.
- **Syntax colour:** One lexer paints SQL (editor, Executed SQL, and fenced
  code in the conversation) and the graph's rendered expressions (summaries,
  facts, rules), with ligatures off. Keywords take the accent at medium
  weight, functions the Model tint, literals the Media amber mixed 78% toward
  ink, operators soft ink, punctuation faint ink, and comments italic faint
  ink; fields stay in ink (medium weight inside stage summaries) so the
  author's own names read strongest. Every kind also differs in weight or
  style, and the text stays complete and copyable. The SQL editor is a
  coloured copy under a transparent textarea with identical metrics: 12px
  mono at 1.6, 12px/16px padding, a 2-space tab.
- **Results:** One 32px run strip: phase with its 6px dot (green complete,
  pulsing accent while running or validating, red failed, ochre
  interrupted), the run selector only when more than one run exists, the row
  count as 12px semibold mono, Spiral's elapsed time in 11px faint mono, a
  live accent elapsed while running, the Truncated chip when it applies, and
  an Executed SQL disclosure at the right that expands inline across the
  strip. While a run is admitted the body shows shimmering skeleton rows in
  the table's own geometry: 60px rows entering 40ms apart, 10px bars sweeping
  recess-to-hairline every 1.4s, and an 88×48 block where the preview will
  land. Stale and failed observations remain visible above the rows on the
  ochre wash.
- **Result table (shared with Data):** A 28px tools row names the column
  count and offers a Types toggle. Headers are sticky paper, 11px medium soft
  ink, with the hairline drawn as an inset shadow so it travels with the
  header. Cells are 12px with 12px/6px padding and a 22rem ceiling. Cells are
  painted by kind: image previews (88×50 on the camera mat, 3px radius, a 2px
  accent ring on hover or focus) lead so five rows stay in view, with the full
  16:9 frame in the inspector; repeated short strings become 11px medium chips
  whose wash is the hue at 13% and whose text is the hue mixed 55% toward ink;
  varying numbers carry a 2px bar (accent at 55%) between the column's
  smallest and largest returned value, explained in the tooltip, and a column
  with both signs anchors its bars at a 1px faint-ink zero mark so negatives
  reach left in graphite (soft ink at 60%) while positives reach right in the
  accent; numbers and timestamps are right-aligned 11px tabular mono;
  timestamps read as `YYYY-MM-DD HH:MM:SS.mmm` UTC in soft ink with the raw
  value in the tooltip; identifiers and paths sit last in faint mono. Rows
  enter 260ms each and 14ms apart, hover washes the row, and any cell click
  opens the inspector. The inspector is the shared side sheet (see "Side
  Sheet" above) headed "Row N" with a faint "of M": images at 16:9
  (`query-row-inspector-media`), then exact values in 11px mono under 10px
  mono field names (`query-row-inspector-facts`); those two body classes
  remain Data's. Up and Down arrows step rows; Escape closes and returns
  focus to the invoking action. No count, class, or total is inferred by the
  presentation.
- **Empty workflow:** The Assistant does the teaching ("What should we find?"
  with two example asks); Graph, Details, and Results each show one faint
  11px line saying what will appear there, with no icon or title, so a new
  workflow is not four onboarding cards.
- **Compare:** The same 36px bar carries the switch, then "A [workflow]
  against B [workflow]" as two 24px selects with 11px keys, and the filled
  Compare action at the right (Analyzing with a spinner while it runs). The
  progress card reads at bench scale: a 12px semibold title with a pulsing
  accent dot and a mono elapsed clock, one 11px line of detail, four hairline
  steps in 11px with 16px marks, and a 2px measurement bar. Metric cards use
  12px padding and 12px gaps, means at the 16px mono readout size, a 64px
  histogram, and 10px footers; measurements that could not be calculated
  share one dashed card that lists their names as chips and each distinct
  error once behind a disclosure. Workflow A follows the workflow open in
  Build, including one saved a moment ago, until the engineer picks another.
- **Assistant column:** The thread and composer share the column, capped at a
  76ch measure when docked wide. Every message names its speaker in a 10px
  medium faint label: your requests sit right-aligned on the recess at up to
  85% width under "You"; answers read at 12px/1.6 on paper beside an 18px
  Model-tinted conversation tile under "Assistant"; Studio notices share the
  tile in graphite on the hover wash; findings inside a turn read at 11px soft
  ink; tool receipts inside the activity fold are 10.5px mono on a hairline.
  Each turn's activity folds behind a disclosure whose 6px dot pulses while a
  tool runs; inside, steps stand on a left hairline with strong-line, good,
  accent, or danger dots. The composer is one bordered field with a filled
  24px square send (an up arrow) or an outline stop at bottom right, an accent
  border plus a 2px 15% accent ring on focus, and a 10px meta line: presence
  dot with the Assistant's state at left, the keyboard hint at right. "Jump to
  latest" floats above the composer with a small lift when the reader has
  scrolled away. The empty thread offers example asks as outlined 11px chips.
- **SQL:** A full-bleed mono textarea; validation issues appear in a danger
  strip beneath it. Run submits exactly this text without saving it.
- **Ownership:** The draft drives the graph. The selected immutable run drives
  Results and Executed SQL. Chat prose cannot override either. The browser's
  existing state controller owns all document, proposal, and run transitions.
  Refer to `docs/workflow-authoring.md` for the current native SQL boundary.
- **Motion:** Everything in Workflow moves on one easing, `--ease-out-expo`
  (`cubic-bezier(0.16, 1, 0.3, 1)`): settle-ins of 160–220ms, the 140ms
  popover, 320ms stage entry, 360ms edge entry, 260ms row entry, 180ms
  inspector entry. Only work in progress loops: the 900ms pulse on run,
  activity, and presence dots, the 700ms edge dashes, the 1.4s skeleton
  shimmer, and the 900ms spinner. All of it is removed under
  `prefers-reduced-motion`.

### Curation Workbench

**Mark the contact sheet.** The whole dataset is visible at once; the recipe
says in words why an episode leaves, and the sheet shows the consequence.
Rule identity is one of the six category washes and is never the only
carrier: every reason chip prints the rule's own words. Nothing on the
surface is said twice.

- **Composition:** The shared 36px document bar (glyph, name ▾, state chip,
  Discard while dirty, Save, filled ▶Run), then the `curation-sheet-v2`
  scope: the 26% Recipe column beside the Sheet. Recipe holds Source,
  Measurements, Rules, and Transforms as hairline-separated sections; the
  Sheet is a run strip over the tile grid, with the selected episode's
  evidence docked at its right edge in the shared side sheet. There is no
  dataset chip in the bar: the dataset is named once, in Source.
- **Document actions:** Save is disabled only when there is nothing to save
  (tooltip "Everything is saved."), while loading, or on a revision
  conflict, which swaps Discard for an outline "Reload current". Run stays
  enabled while the recipe is merely incomplete; pressing it reports what is
  missing in the status bar and the same words sit on Run's tooltip. Run
  disables only for a load error, a pending command, or a run in progress
  (spinner + "Judging"). ⌘/Ctrl+Enter runs from anywhere in the workspace.
- **Recipe section:** An 11px medium soft-ink title with the count in 10px
  faint mono at the right, 10px/12px/12px padding, a lower hairline. A
  section's one add control is a 24px select ("Add a measurement…"), so a
  section grows by one line rather than a row of buttons; Rules alone adds
  with an outline "Add rule". Notes are 11px faint at 16px leading; an alert
  note turns brick red. Source's facts (episode count, recorded time) are
  one 10px soft-ink mono line; the source caution ("Dataset list unavailable
  · type its catalog name") is one truncating 10px ochre line. The Transforms
  note shows only while the section is empty.
- **Measurement and transform rows:** One line each (`curation-fact`),
  separated by hairlines, 6px vertical padding: a 12px chevron, the 12px
  medium label, then what tells it from another of its kind in 10px faint
  mono (`curation-fact-detail`, truncating), and the remove action at the
  right. No kind chip. The row opens when it still needs something or when
  the operator asks; the open body is an `auto minmax(0,1fr)` label/value
  grid in 11px soft ink over 24px fields.
- **Rule sentence:** Two lines (`curation-rule`, 6px padding, 4px gap). Line
  one: the mark · "If" · fact select (≥ 5.5rem, grows) · the comparison
  grouped in `curation-condition-test` (op ≤ 7.5rem, value 3.5rem) so a
  wrap breaks before the comparison, never inside it. Line two: "exclude
  as" · reason input (≥ 6rem, grows) · the count ("N removed", 11px
  semibold mono over 10px faint) · remove. Words are 11px faint ink; blanks
  are 24px fields. Every builtin fact sits in the fact select under "Every
  episode"; nothing lists them elsewhere.
- **Rule mark:** A 16px round transparent button (`curation-rule-mark`) is
  the rule's handle and its switch. It holds a 6px dot filled in the rule's
  tint with a 1px ring of the same tint; `aria-pressed` reports the rule
  active. A paused rule keeps the ring and loses the fill, and its words
  fade to 0.55. Hover scales the dot 1.35 over 120ms. Hovering or focusing
  the mark, or the "N removed" count, lights the rule's tiles on the sheet
  and washes the rule row with the hover tone over 100ms; the words beside
  the mark do not light anything, so editing leaves the sheet alone.
- **Run strip:** Phase with its dot; a `run-select` of runs only when there
  is more than one; the All · Kept · Excluded filter as Workflow's 22px
  compact segments, each carrying its total as a `strong` 11px semibold mono
  count, so the strip has no separate totals line; the lens "Tiles
  ⟨measurement ▾⟩" (`curation-lens`, faint word + `run-select`) beside a
  three-state quiet order button (`curation-order`: Dataset order · Lowest
  first · Highest first); Spiral's judging time; a caution chip "Sheet
  truncated" when the list is bounded; the executed SQL. A stale run keeps
  Workflow's "Previous run" notice.
- **Contact-sheet tile:** `minmax(11rem, 1fr)` grid, 6px gap, paper with a
  1px hairline, 4px radius, 6px/8px padding, 4px internal gap. Head: the
  ordinal in 11px medium soft-ink mono (2ch minimum; the one certain
  identity on a sheet where task names repeat), then the 12px medium name
  clamped to two lines. Line: the 10px soft mono clock at the left and the
  lensed value at the right (≤ 62%, truncating). Duration is the tile's own
  baseline: a 2px strong-line rule along the bottom edge whose width is the
  episode's fill against the sheet's longest, drawn on kept tiles only and
  animating its width over 260ms. Hover darkens the hairline to strong-line;
  the selected tile takes the accent hairline and a 1px inset accent ring.
- **Struck tile:** On the recess with faint ink for name and clock, the name
  at regular weight, the hairline mixed 45% toward the rule's tint, no
  baseline, and the reason chip given the whole second line so it may wrap.
  A struck tile keeps its name legible: exclusion is a decision, not an
  error.
- **Reason chip:** 10px medium, 4px radius, 1px/6px padding, the rule's hue
  at 13% wash with text mixed 55% toward ink (the result-chip recipe). More
  than one reason adds a "+N" in faint mono.
- **Hover sweep:** While a rule's mark or count is hovered, the tiles it
  struck take a 1px inset ring in its tint and every other tile fades to
  opacity 0.32, so the sheet becomes the rule's answer.
- **Skeletons:** Tiles of 3.5rem minimum height shimmering recess → hover →
  recess over 1.4s, entering 40ms apart, until the run answers.
- **Episode evidence:** Inside the shared side sheet headed "Episode N" with
  a faint "of M", the body (`curation-episode`, 12px padding, 12px gaps) is:
  a 6px status dot (connection green for kept, faint ink for excluded) with
  the 11px medium verdict word, the task label as a 12px semibold `h4`, the
  source path in 10px faint mono; a stat grid (`minmax(6.5rem, 1fr)`) with
  10px faint terms and 12px mono values; then evidence blocks under 11px
  medium soft-ink titles: Measurements as an `auto minmax(0,1fr)` key–value
  list where the key never truncates and the mono value truncates at the
  right; Rules as verdict rows led by the rule dot, ink when fired and faint
  when clear, with "excluded" / "clear" in 10px mono; Transforms the same
  way ("applies" / "skipped" / "not kept"). Sampled frames load the moment
  the sheet opens (a spinner line "Decoding N sampled frames…" while they
  decode) as full-width 16:9 figures on the camera mat with 10px mono
  captions.
- **Status bar:** connection · dataset · phase only while Running or Failed
  · the accent verdict "N of M kept".
- **Motion:** Tiles enter over 260ms on `--ease-out-expo` with a 3px rise,
  staggered 12ms; opacity, ring, and hairline changes settle in 160ms on the
  same easing; the dot's fill and scale in 120ms; only the skeleton shimmer
  loops; all of it is removed under `prefers-reduced-motion`.

**The Struck-Not-Erased Rule.** An excluded episode stays on the sheet with
its name legible and the reason in words. Exclusion dims and tints; it never
hides, shrinks, or strikes the text through.

**The Said-Once Rule.** A fact has one home on the Curation surface. The
dataset is named in Source, not in the document bar or a chip; the totals
live in the filter segments, not in a count line; the phase appears in the
status bar only while it is news and yields to the verdict when the run
completes; a run is numbered only once there is more than one. Do not add a
second reading of a value that is already on the page.

### Timeline

- **Transport:** Record, Pause/Resume, Stop/Discard, one icon-only Play/Pause,
  **Sync robot**, and the 16px clock occupy one stable toolbar. Record always
  implies Teleop. Play/Pause affects visualizations only until robot sync is
  explicitly enabled.
- **Playhead:** One pointer and keyboard-operable time cursor drives every
  recorded surface. Scrubbing updates camera, scene, and signals; with robot
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
30% left rail separates **Setup** above **Runs**. The
remaining workspace is one tab group: every opened run owns one tab containing
its overview, dominant learning curve, data path, throughput, events, and logs.
Opening history never replaces or rewrites the new-run draft.

- **Control and ledger:** Setup retains dataset, device, run settings, and a
  collapsed **Advanced settings** disclosure. Its single filled **Run training**
  action stays visible beneath the independently scrolling form, disabled
  beside **MCAP training is not available yet** and the native Spiral decoding
  blocker. Recording and Data playback remain available. Runs is a separate
  newest-first history ledger; existing run details and checkpoint downloads
  remain accessible. An active run retains progress and **Stop training**.
- **Run tabs:** Selecting a history row appends or activates that immutable run
  in the right tab group; already opened runs remain available as sibling tabs.
  Each run tab can be closed without stopping or deleting the backend run.
  Closing the selected tab activates the run to its right, then the one to its
  left; closing the last run restores the empty **Run** panel.
  Current MCAP camera datasets cannot start new training runs.

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
  context, while Stop remains reachable for the run in progress. The MCAP
  capability blocker takes precedence over compute readiness; connecting a
  runtime does not enable training before native Spiral decoding is supported.
  A failed run keeps its terminal reason and the latest training log lines visible in its run tab.
- **Owned data path:** Run tabs preserve recorded data-path stages and measured
  tensor/optimizer throughput as historical evidence. The legacy registry/MP4
  training loader is removed; no MCAP decoder fallback supplies training
  images. Runtime connection and retained history must not imply that new
  dataset training is available.
- **Terse operating copy:** Panel tabs name Setup, Runs, and each run; run
  content uses the short evidence headers **Loss**, **Data path**, and
  **Throughput**. Configuration uses field labels, run state uses measured
  facts, and secondary configuration stays under **Advanced settings**.
  Data-path stages pair short noun labels with recorded values instead of
  architecture prose.
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
  Checkpoint schema versions must match exactly: schema-10 checkpoints remain
  available for inspection/download, but cannot run after the schema-11 migration.
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
  or tabbing a panel must not fabricate a second connection. Recorded H.264
  frames use native WebCodecs through Rust/WASM and seek from the preceding
  keyframe to the episode playhead.
- **Recorded state:** Source fetching, decoding, and missing coverage are
  explicit inside the camera mat. During a keyframe interval load show
  “Decoding recorded camera…”; before the first frame show “No camera frame at
  this time.” Decode and fetch failures remain legible instead of presenting
  an unexplained blank image or substituting live pixels.
- **Failure:** Actionable unavailable/permission text appears in the frame; a
  green dot never masks a blank image.

### Data, Episodes, and Query

- **Datasets rail:** Datasets lead; each expands into chronological episode
  rows of `#n`, the distinguishing directory of the source file, duration,
  date, and message count, fifty at a time with an explicit Load more. The
  selected dataset shows its episode count; the open episode is role blue.
  Tables and views sit in a collapsed Tables & views
  section; opening one loads `SELECT * … LIMIT 100` into SQL and runs it.
- **Overview:** Selecting a dataset shows its name, chips for episode count,
  total recorded time, total bytes, and date span from one aggregate query,
  its declared streams with schema names and counts, and two actions: Open
  first episode and Query episodes in SQL.
- **Timeline:** A 36px transport row holds the episode identity, jump/step/
  play controls, a millisecond clock beside the duration, speed, Loop, a
  buffering note, and zoom. Below it a sticky ruler with round-interval ticks
  sits over one 28px row per stream: expand chevron, a show checkbox (mixed
  when only some fields are plotted), kind glyph, name, message count, and a
  density track of 240 buckets tinted by relative count. Expanded data
  streams list their fields with type labels and per-field checkboxes. The
  playhead crosses ruler and rows; Space, arrows, Home/End, `+`/`−`/`0`, and
  Ctrl/⌘-wheel operate the clock and zoom.
- **Plots:** One section per plotted stream and one 96px lane per field, with
  a labelled value axis and one coloured trace per array index. The readout
  column doubles as the legend: swatch, index, and the value at the playhead
  or under the hover cursor; clicking a trace hides it. A click seeks. A
  whole episode that exceeds the read bounds falls back to the visible span
  and says so. The empty state offers one-click plots of the busiest streams.
- **Cameras:** Each video stream is its own camera panel in the right-hand
  column, mounted only while open; the frame keeps the dark mat and the
  recorded frame time. Closing a panel unticks the stream; the Timeline
  reopens it.
- **Messages:** The selected stream's messages from the playhead forward in
  the shared result table, one hundred at a time, with From playhead and
  Next 100 actions.
- **SQL:** The read-only editor above its bounded results, as one tab beside
  Plots and Messages; it retains its draft and result tabs across the
  session.
- **Import:** A peer native tab contains a restrained form leading from source
  fields to inspection, a resolved-revision review, import, and a success
  receipt. Finishing selects the new dataset in the rail.
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
Calibration applies to future recordings; there is no action to rewrite saved
MCAP episodes. Project transfer uses format 2 and restores original MCAP bytes
and Dataset names with Studio state; format-1 backups require the old application.

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

**The Display-Only-Inference Rule.** A cell's kind (image, chip, bar, clock
time, identity) changes how a stored value is drawn, never what it is. The
exact value stays in the tooltip and the row inspector, and the presentation
never invents a count, class, or total.

**The Hue-Plus-Word Rule.** A stage tint, category wash, or syntax colour is
always paired with a word, glyph, weight, or style that carries the same
meaning. Remove the colour and the graph, table, and SQL still read.

**The One-Easing Rule.** Studio motion uses `--ease-out-expo` for entrances
and settle-ins; durations stay under 400ms; loops exist only while work is
in progress; and every animation is removed under `prefers-reduced-motion`.

## Do's and Don'ts

### Do:

- **Do** keep shell, panels, WebGL, plots, Timeline, settings, and data tools in
  one light neutral world; reserve the dark mat for camera imagery.
- **Do** register independent surfaces as panels and give them durable IDs and
  meaningful home tiles.
- **Do** preserve Live's one-third/two-thirds default, Data's 21% rail / 79%
  native object panels default, and Rollout's 27% workbench default while treating
  persisted user layout as the active composition.
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
- **Do** keep Live, Data, Workflow, Curation, Training, and Rollout layout
  state independent, and keep episode selection and camera ownership
  activity-scoped so switching cannot move content between them.
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
- **Do** keep Workflow graph-first: a 24% Assistant/SQL console, the Graph
  (74%) beside Stage details (26%), and Results in the lower 54%, with stage
  cards fixed at 224×92 and result previews at 88×50.
- **Do** paint result cells by kind through the shared table and lexer:
  category chips at 13% wash, 2px measure bars anchored at zero when a column
  crosses it, UTC clock time, faint mono identity, and one syntax palette for
  SQL and rendered expressions.
- **Do** keep Curation a contact sheet: a 26% Recipe column beside the
  Sheet, one tile per episode in a `minmax(11rem, 1fr)` grid, struck tiles
  on the recess with the rule's words in the rule's tint, the selected
  episode's evidence docked in the shared side sheet, and the shared document
  bar and run strip in the same place as Workflow's.
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
- **Don't** add fixed one-off panel arrangements in the Studio; take a generic
  mechanic upstream to [`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench)
  and register domain content in the Studio.
- **Don't** name product components or local CSS classes after native
  presentation such as `Button`; name the action's domain intent or the layout
  responsibility.
- **Don't** turn every tool into primary navigation. Live, Data, Workflow,
  Curation, Training, and Rollout are activities; panel tabs are spatial
  grouping inside them.
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
- **Don't** let a stage or category hue stand alone, restate the six tints as
  health, or add a seventh kind without a glyph and a one-word label.
- **Don't** add a second easing curve, a looping animation that outlives the
  work it reports, or a resting shadow on anything that is not standing on the
  graph bench.
- **Don't** rebuild Curation as a query builder over a results table, hide
  excluded episodes from the sheet, deal a rule tint that reads as role blue
  first, let a tint stand without the rule's words on the tile, or give the
  selected episode a third panel that sits empty at rest.
