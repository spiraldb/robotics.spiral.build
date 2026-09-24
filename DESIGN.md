---
name: Spiral Robotics Studio
description: A precise, catalog-first robot-data instrument for operating robots, understanding episodes, curating them into kept sets, training policies, and staging fixed-follower rollouts.
colors:
  canvas: "#f7f7f5"
  surface: "#ffffff"
  sidebar: "#fbfbfa"
  sunken: "#f5f5f2"
  viewport: "#f4f4f1"
  camera: "#141414"
  hover: "rgb(16 16 16 / 0.045)"
  pressed: "rgb(16 16 16 / 0.085)"
  line: "#e2e2de"
  line-soft: "#ececea"
  line-strong: "#c9c9c4"
  ink: "#121212"
  ink-soft: "#4d4d4a"
  ink-faint: "#6e6e6a"
  primary: "#121212"
  primary-hover: "#2b2b2a"
  primary-ink: "#ffffff"
  accent: "#cee562"
  accent-strong: "#a3bc2a"
  accent-wash: "#f2f8d6"
  accent-line: "#d6e595"
  accent-ink: "#4a5610"
  selected: "#eef5cf"
  info: "#5971fd"
  info-ink: "#3a4ec4"
  info-wash: "#edf0ff"
  info-line: "#d8defe"
  danger: "#c4302b"
  danger-soft: "#fbeaea"
  danger-line: "#f1cfcd"
  caution: "#a64b13"
  caution-soft: "#fff1e8"
  caution-line: "#f6d6c2"
  good: "#5b7400"
  good-dot: "#84a00c"
  good-soft: "#f2f8d6"
  kind-dataset: "#1fa3bb"
  kind-episode: "#5971fd"
  kind-stream: "#7a5ad6"
  kind-camera: "#e0752f"
  stage-source: "#5971fd"
  stage-media: "#e0752f"
  stage-model: "#7a5ad6"
  stage-shape: "#6e6e6a"
  stage-join: "#1fa3bb"
  stage-output: "#5b7400"
  category-1: "#5971fd"
  category-2: "#f07a36"
  category-3: "#1fa3bb"
  category-4: "#c98f00"
  category-5: "#d9679a"
  category-6: "#7a5ad6"
  series-train: "#5971fd"
  series-validation: "#f07a36"
  series-memory: "#7a5ad6"
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
typography:
  title:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "19px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.025em"
  heading:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "12.5px"
    fontWeight: 600
  dialog-title:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 600
    letterSpacing: "-0.01em"
  body:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.45
  secondary:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 400
  action:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: 1.2
  current:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 560
  meta:
    fontFamily: "Geist Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "11px"
    fontWeight: 400
    fontFeature: "tabular-nums"
  label:
    fontFamily: "Geist Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "10.5px"
    fontWeight: 500
    letterSpacing: "0.03em"
  readout:
    fontFamily: "Geist Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "16px"
    fontWeight: 500
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "tabular-nums"
rounded:
  chip: "4px"
  row: "5px"
  control: "6px"
  card: "8px"
  dialog: "10px"
  full: "999px"
spacing:
  xxs: "2px"
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "12px"
  xl: "16px"
  xxl: "24px"
  target-compact: "26px"
  target-row: "28px"
  target-standard: "30px"
  target-prominent: "34px"
  tab-bar: "34px"
  subject-head: "40px"
  top-bar: "44px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-ink}"
    typography: "{typography.action}"
    rounded: "{rounded.control}"
    padding: "0 10px"
    height: "{spacing.target-standard}"
  button-primary-hover:
    backgroundColor: "{colors.primary-hover}"
    textColor: "{colors.primary-ink}"
  button-outline:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.action}"
    rounded: "{rounded.control}"
    padding: "0 8px"
    height: "{spacing.target-compact}"
  button-quiet:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.action}"
    rounded: "{rounded.control}"
    padding: "0 8px"
    height: "{spacing.target-compact}"
  button-quiet-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.ink}"
  button-icon:
    backgroundColor: "transparent"
    textColor: "{colors.ink-faint}"
    rounded: "{rounded.control}"
    size: "{spacing.target-compact}"
  button-pressed:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink}"
  button-selected:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.accent-ink}"
  segment-track:
    backgroundColor: "{colors.sunken}"
    rounded: "{rounded.card}"
    padding: "2px"
  segment-checked:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.row}"
    height: "28px"
  field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "0 8px"
    height: "{spacing.target-compact}"
  chip:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.meta}"
    rounded: "{rounded.chip}"
    padding: "0 6px"
    height: "20px"
  chip-accent:
    backgroundColor: "{colors.accent-wash}"
    textColor: "{colors.accent-ink}"
  chip-caution:
    backgroundColor: "{colors.caution-soft}"
    textColor: "{colors.caution}"
  chip-danger:
    backgroundColor: "{colors.danger-soft}"
    textColor: "{colors.danger}"
  kbd:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink-faint}"
    rounded: "{rounded.chip}"
    padding: "0 4px"
    height: "18px"
  kind-mark:
    backgroundColor: "transparent"
    textColor: "{colors.kind-dataset}"
    rounded: "{rounded.chip}"
    size: "16px"
  tree-row:
    backgroundColor: "transparent"
    textColor: "{colors.ink-soft}"
    typography: "{typography.body}"
    rounded: "{rounded.row}"
    height: "{spacing.target-row}"
  tree-row-selected:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.ink}"
  top-bar:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    height: "{spacing.top-bar}"
  nav-item:
    backgroundColor: "transparent"
    textColor: "{colors.ink-faint}"
    typography: "{typography.body}"
    padding: "0 10px"
    height: "{spacing.top-bar}"
  nav-item-current:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.current}"
  status-line:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-faint}"
    typography: "{typography.meta}"
    height: "{spacing.top-bar}"
  panel-tab-bar:
    backgroundColor: "{colors.sidebar}"
    textColor: "{colors.ink-faint}"
    typography: "{typography.secondary}"
    height: "{spacing.tab-bar}"
  panel-tab-current:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    padding: "0 11px"
  catalog-rail:
    backgroundColor: "{colors.sidebar}"
    textColor: "{colors.ink}"
    width: "21%"
  subject-head:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    padding: "0 12px"
    height: "{spacing.subject-head}"
  ledger-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
  ledger-header:
    backgroundColor: "{colors.sunken}"
    textColor: "{colors.ink-faint}"
    typography: "{typography.label}"
    padding: "8px 12px"
  stage-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "10px 12px 8px"
    width: "224px"
    height: "92px"
  stage-mark:
    backgroundColor: "transparent"
    textColor: "{colors.stage-source}"
    rounded: "{rounded.row}"
    size: "18px"
  curation-node-bar:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.secondary}"
    padding: "5px 12px"
    height: "40px"
  curation-index-row:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.secondary}"
    padding: "4px 12px"
    height: "62px"
  curation-index-row-chosen:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.ink}"
  side-sheet:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    width: "320px"
  popover:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.dialog}"
    width: "320px"
  dialog:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.dialog-title}"
    rounded: "{rounded.dialog}"
  toast:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-soft}"
    typography: "{typography.secondary}"
    rounded: "{rounded.card}"
    padding: "10px 14px"
  transport-clock:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.readout}"
    width: "11rem"
---

# Design System: Spiral Robotics Studio

## Overview

**Creative North Star: "Ink on the Bench Ledger"**

Studio is a precise, catalog-first instrument for robot data: ink on paper,
hairlines for structure, mono for every measurement, and one lime mark that
says "you are here". Paper panels sit on a warm off-white canvas; rails and
tab bars take a half-step sidebar tone so a place reads as a place without
becoming a grey slab. Camera pixels alone keep a dark mat for honest
letterboxing. It replaced a washed-out grey workbench with blue selection, a
left activity rail, and a bottom status bar; none of those return.

The Studio has six activities, reached from one 44px top bar and the keys
1–6 in this order: **Data** (catalog, episodes, import; the home route),
**Workflow** (deriving and querying Spiral relations: Explore and Agent),
**Curation** (a graph of measurements and selections over one dataset's
episodes, ending in the parts worth training on),
**Training** (configuring and observing policy runs), **Rollout** (staging a
trained checkpoint against the configured follower), and **Live** (operating
hardware and recording). Everything else is a panel in a session-owned
tab-and-tile workspace (`dioxus-workbench`) that docks, tabs, splits, and
resizes with pointer and keyboard. The layout is powerful; the chrome stays
quiet.

Density reads as precision only if the surface holds still. The body is 13px
Geist; measurements, ids, counts, and durations are Geist Mono with tabular
figures; labels over values are small upper-case mono. Colour is spent on
selection (lime), the one primary action (ink), focus and links (blue), real
state (caution, danger, good), and identity (kind, stage, arm, signal hues).
Telemetry arrives continuously, so live numbers reserve their widest width,
status reports the exception rather than the resting state, and no sentence
unfolds under a changing value.

**Key Characteristics:**

- One light world: paper panels on the canvas, sidebar-toned rails and tab
  bars, a pale 3D bench; the dark mat is for camera pixels only.
- A 44px top bar owns the brand, the activity nav, one status line, and the
  shortcuts, notifications, and settings actions. There is no side rail and
  no bottom status bar.
- Lime marks only the current selection; ink marks the current tab and fills
  the one primary action; blue is focus, links, and work in progress.
- Geist 13px body, 19px subject titles, Geist Mono for every measurement,
  one 16px mono elapsed readout as the visual maximum.
- Instrument strips: small upper-case labels over mono values between
  hairlines, used for stat strips, ledger headers, and readouts.
- Letter kind marks (D dataset, E episode) and outlined stage glyphs name what
  a thing is before its name is read.
- Gently rounded forms on a 4/5/6/8/10px ladder; soft blurred shadows only on
  things that float or stand on the graph bench.
- One easing (expo out) for every entrance; reduced motion keeps only fades.
- Desktop-only at 1024 CSS pixels and wider; the persisted split tree is
  always the rendered workspace.

## Colors

Warm paper and near-black ink with graphite hairlines, one lime selection
mark, one blue for focus, and semantic washes that each carry a matching
line colour.

### Primary

- **Ink** (`{colors.primary}`, hover `{colors.primary-hover}`): The filled
  primary action of a region (Run, Run training, Run policy), the current activity's underline in the top bar, the
  current tab's underline in a multi-tab panel strip, the transport's round
  play button, and a selected episode's length bar. Ink is also the text
  colour, so the primary action reads as the strongest word on the page, not
  as a hue.
- **Selection Lime** (`{colors.accent}`): Where you are, and nothing else. It
  fills the selected catalog tree row and the current Settings section, dots
  the notification count, and rings a selected stage card or contact-sheet
  tile (`{colors.accent-strong}` hairline plus a 3px lime halo at 45%). Its
  quieter forms carry selection inside lists and buttons: the
  `{colors.selected}` wash with `{colors.accent-ink}` text for a selected
  list button, `{colors.accent-wash}` with `{colors.accent-line}` for an
  outlined one and for accent chips. The dock preview while dragging a tab is
  lime at 22% behind an accent-strong edge.

### Secondary

- **Focus Blue** (`{colors.info}`, text `{colors.info-ink}`): The 2px
  keyboard focus outline everywhere (field focus adds a 3px halo at 18%),
  text links and quiet text actions, and work in progress: the pulsing run
  and tool dots and the marching edges of a running workflow graph. It never
  marks selection.
- **Danger Red** (`{colors.danger}` on `{colors.danger-soft}` behind
  `{colors.danger-line}`): Destructive consequences, recording, faults,
  energized torque, reconnecting. In the status line a danger fact is red and
  medium-weight; the connection dot pulses while reconnecting.
- **Caution Rust** (`{colors.caution}` on `{colors.caution-soft}` behind
  `{colors.caution-line}`): Conditions that need attention without meaning
  failure: simulation, degraded rates, robot setup loading or unavailable, a
  stream missing from some episodes in the streams ledger, stale results.
- **Good Olive** (`{colors.good}`, dot `{colors.good-dot}`, wash
  `{colors.good-soft}`): Connected, saved, complete. It paints small dots and
  the good chip (which shares the lime wash); it never fills a control.

### Tertiary

- **Kind hues** (`{colors.kind-dataset}`, `{colors.kind-episode}`,
  `{colors.kind-stream}`, `{colors.kind-camera}`): The outline and letter of a
  kind mark, and the camera glyph in stream lists. Identity, not state.
- **Stage kinds** (`{colors.stage-source}` … `{colors.stage-output}`): What a
  workflow stage does to the data. Each paints only the stage's outlined
  glyph mark, the one-word upper-case kind in the card footer, and a 14% wash
  where a tile needs one.
- **Result categories** (`{colors.category-1}` … `{colors.category-6}`):
  Washes for repeated string values in a result table, assigned by rank
  within that result: hue at 13% wash, text mixed 55% toward ink. Curation
  deals the same six to a grouping's categories, biggest first, as
  `category-2, 4, 3, 5, 6, 1`, so blue comes last; a search's matches wear
  its node's own `stage-media` hue rather than focus blue.
- **Arm identity** (`{colors.arm-1}` … `{colors.arm-4}`), **joint signals**
  (`{colors.signal-1}` … `{colors.signal-6}`, by joint name in kinematic
  order), **axis key**, and **training series** (`{colors.series-train}`,
  `{colors.series-validation}`, `{colors.series-memory}`): renderer-owned
  identity colours. Every plotted value is also available as text.

### Neutral

- **Canvas** (`{colors.canvas}`): The ground under the workspace.
- **Paper** (`{colors.surface}`): Panel content, the top bar, cards, fields,
  popovers, dialogs, toasts, the current panel tab.
- **Sidebar** (`{colors.sidebar}`): The catalog rail, panel tab bars,
  Settings navigation, dialog footers, the Data overview footer, ledger row
  hover.
- **Recess** (`{colors.sunken}`): Pressed-in toggles, segment tracks, ledger
  headers, kbd chips, disabled fields.
- **Pale Bench** (`{colors.viewport}`): The 3D stage and graph canvas.
- **Camera Mat** (`{colors.camera}`): Camera frames and their loading and
  failure states only.
- **Hairlines** (`{colors.line}` → `{colors.line-soft}` →
  `{colors.line-strong}`): Panel and control boundaries; the softer rule
  inside strips and ledgers; the firmer edge an outlined control takes on
  hover.
- **Graphite ramp** (`{colors.ink}` → `{colors.ink-soft}` →
  `{colors.ink-faint}`): Primary text, secondary text, metadata. Faint ink
  clears 4.5:1 on the recess.
- **Hover / Pressed** (`{colors.hover}`, `{colors.pressed}`): One tonal step
  for the pointer, one more for a held press or the hover of a pressed
  toggle. Nothing moves.

### Named Rules

**The Lime-Is-Where-You-Are Rule.** Lime marks the current selection only: a
selected row, section, card, or tile. It is never focus, never a primary
action, never a link, never decoration, and never a tab indicator. If the
thing is not the operator's current selection, it is not lime.

**The Three-Marks Rule.** Selection is lime, the current tab and the primary
action are ink, focus is blue. The three never trade places, so a screen can
show all three at once without ambiguity.

**The Wash-Plus-Line Rule.** A semantic state is its ink colour on its soft
wash behind its own line colour (`caution` / `caution-soft` /
`caution-line`, and the same for danger, accent, info). A state never borrows
another state's line or uses a neutral hairline with a coloured wash.

**The One-Spelling Rule.** Colours are spelled once, as role-named `--tone-*`
properties in `crates/frontend/tailwind.css`. The kit reads them through
`--ui-*` (its fallbacks mirror the same literals), the workbench crate
through `--wb-*`, and charts through `--chart-*` read off the root. Arm and
signal colours stay with the renderer code that paints them.

**The Blue-Comes-Last Rule.** Identity tints dealt from the category washes
start away from blue and reach it last, because blue already means focus.

## Typography

**Body Font:** Geist (self-hosted variable woff2, OFL, weights 100–900), with
`-apple-system`, BlinkMacSystemFont, Segoe UI, sans-serif.

**Label/Mono Font:** Geist Mono (self-hosted), with `ui-monospace`, SF Mono,
Menlo, monospace.

**Character:** Geist is a crisp, slightly technical grotesk that stays calm at
13px; Geist Mono is its measured twin, so a value beside a label reads as the
same voice switching to data. Both are installed by the UI kit
(`crates/ui/src/foundation/styles.rs`), so no surface depends on a system
face.

### Hierarchy

- **Title** (600, 19px, -0.025em): A subject's name: the "Datasets" home, a
  dataset's Overview heading beside its 30px kind mark, the Import pane,
  Settings. One per subject.
- **Heading** (600, 12.5–13px): Section names inside a subject ("Episode
  lengths", "Streams"), the episode subject path, empty-state titles (13px).
  **Dialog title** (600, 14px, -0.01em) is the one step between Heading and
  Title: the confirmation dialog and the shortcut sheet.
- **Body** (400, 13px, ~1.45): Tree labels, fields, panel content,
  sentences. **Current** (560) is the weight a thing takes when it becomes
  the current one: the current nav item, the current tab, a pressed toggle,
  the selected dataset name.
- **Secondary** (400–500, 12–12.5px): Buttons (12px, 500; solid 560), panel
  tab titles, table cells, episode row titles, captions, toasts.
- **Meta** (400, 11–11.5px mono, tabular): Ids, counts, durations, dates,
  paths, schema names, chips, the status line, the subject's facts line.
- **Label** (500–600, 10.5px, upper-case, 0.03–0.07em): Stat-strip labels,
  ledger and table headers (mono), workbench section names, stage kinds.
  Fact tiles use the same treatment at 10px.
- **Readout** (500, 16px mono, -0.02em, tabular): The transport's elapsed
  clock, reserving 11rem. Nothing else is set this large inside a panel.

### Named Rules

**The Mono-Measures Rule.** Every value that is a measurement or an
identifier (duration, count, size, rate, time, id, path, schema) is Geist
Mono with tabular figures. Words stay in Geist.

**The Still-Numbers Rule.** Tabular figures equalize digit widths, not digit
counts. Every telemetry-fed value also reserves space for its widest expected
reading so nothing beside it moves.

**The Labels-Over-Values Rule.** Upper-case is for a small label that names a
value, a column, or a control group directly beneath or beside it. It never
sits above a larger heading as a kicker or eyebrow, and headings and
sentences stay sentence case.

**The 19px-Ceiling Rule.** Subject titles are the largest text in the Studio
and appear once per subject; the 16px mono clock is the largest thing inside
a panel. Panel titles do not grow because a tile grows.

## Layout

The first viewport is a full-height shell:

- a 44px paper top bar in three columns: brand and activity nav at left, the
  status line filling the middle, then a hairline-separated trail of
  shortcuts (`?`), notifications, and Settings (`,`);
- the body below it owned by the active activity's `PanelWorkspace` on the
  canvas;
- transient toasts floating at the lower right, never inserting a row.

The brand is the Spiral mark and the two-weight "spiral robotics" wordmark,
closed by a hairline. The nav lists Data · Workflow · Curation · Training ·
Rollout · Live as glyph-plus-word items the full height of the bar; keys 1–6
open them in that order and `/` routes to Data. Each activity registers its
own panel set and independent session layout scope, so switching activity
never moves content between them.

The status line writes facts most important first and lays them out right to
left inside a clipped, wrapping row: the connection fact (a 7px dot and the
project id, or "Reconnecting" in danger), then the active activity's facts
sorted danger, caution, then plain. When the bar narrows, the least important
facts fall to the hidden second line whole rather than truncating. A fact
that names something to fix is a button that opens the activity that fixes
it.

Panel tab bars are 34px on the sidebar tone; splitters render as a 1px
hairline with an 8px pointer target. Tiles meet at the hairline, so adjacent
tab bars read as one bar interrupted by a rule.

### Data

Data follows the operator's model of their recordings: a dataset is a folder
of MCAP files and each file is one episode. The canonical layout at
1440×900:

```text
┌────────────────────── 44px top bar ─────────────────────────────────────┐
├── 21% ────┬── Overview │ E episode × │ E episode × ─────────────────────┤
│ Datasets  │ D dataset / Episode …id   0:39.9 · 2025-04-14      J K step │
│ Filter [/]│ camera · top │ camera · left-wrist │ camera · right-wrist   │
│ Import    ├───────────────────────────────────────────────────────────────┤
│ D name  40│ Plots — lanes on the Timeline's time axis                    │
│  episodes │───────────────────────────────────────────────────────────────┤
│  …        │ Timeline — transport, ruler, one row per stream              │
└───────────┴───────────────────────────────────────────────────────────────┘
```

- **Catalog rail** (21%, sidebar tone): a filter field with a `/` kbd chip,
  refresh, and an outlined Import; then the dataset tree. A dataset row is a
  D kind mark, the display name, and its episode count in mono. Expanded, it
  lists chronological episode rows that lead with the task label (or the
  distinguishing id when unlabelled), then a mono line of id, date, a 3px
  length bar against the longest episode listed, and the duration. The rail
  marks what is in front: the front episode tab's row, else the Overview's
  dataset.
- **Subject tabs:** one Overview tab (D mark) plus one closable tab per
  opened episode (E mark), in the order opened. J and K step the front
  episode tab on through the dataset rather than opening another; Esc brings
  the dataset forward in Overview. Closing the front tab brings the newest
  remaining episode forward. Import opens as a peer tab in the same tile.
- **Datasets home** (Overview with no dataset): the 19px "Datasets" title and
  one sentence, then the datasets ledger: name with D mark, episodes,
  recorded, average length, size, and date span, rows entering 40ms apart.
- **Dataset overview:** a 30px D mark beside the 19px name, a stat strip
  (Episodes, Recorded, Average length, Size, Streams) between soft hairlines,
  the "Episode lengths" distribution (104px of bars with a dashed average
  marker and a mono axis), the "Streams" ledger, and a sidebar-toned footer
  naming the source in mono (`spql.mcap.episodes('dataset_…')`).
- **Episode subject:** a 40px head naming where it is on one line, whatever
  the label's length: the D mark and dataset (the way back), "/", the
  semibold "Episode …id", the mono facts, and the J K step hint at right.
  Below it, the shared episode inspection.

The inspection arranges itself for the room its surface gives it
(`activities/episode/arrangement.rs`), so it reads the same everywhere. Three
bands share one clock, top to bottom: the views (cameras, the leading one
first, then the Scene), Plots, and the Timeline. The views take only the
height their pictures can use, in the grid that leaves the smallest picture
largest and spends the least on camera mat; the leading camera takes a
column of its own only where that makes its picture the larger. No view is
drawn in a tile smaller than 256×200 px: views that do not fit join the last
tile as tabs. Plots and the Timeline span the same width and give their names
the same column, so one vertical line through both is one instant. A room too
short for a Plots band (under 200 px after the Timeline's 30%) sends Plots to
the last view tile as a tab, never behind the Timeline; asking to plot a
stream brings it forward. Shares move in 2% steps, so a small resize keeps the
operator's arrangement; a different arrangement is a new layout scope. The
inspection keeps at least 30rem of height and its surface scrolls below that.
A camera panel's tab carries the camera glyph and the part of its stream name
that differs from the episode's other cameras ("top", "left-wrist"), so the
name survives a crowded tab strip; the leading camera is the one a surface
names (a measurement's camera), else the overview camera (top, front, head).
Only a camera on screen holds the clock while it loads: one waiting behind
another tab never keeps playback buffering.

When the operator has chosen nothing, Data plots the busiest stream with
numeric fields once every data schema has loaded, so Plots is never empty
when an episode opens. An explicit plot choice, including deselection, wins
over an automatic selection still waiting for a schema response.

The Timeline is the navigation hub of an episode: ticking a stream shows it
(a camera opens its panel; a data stream draws every numeric field in Plots),
and expanding a data stream exposes its fields for one-by-one selection. A
filter field names the stream count. Cameras lead the list, then streams with
numeric fields; streams with at most one message or no numeric fields wait
behind one "N streams with nothing to plot" row (a count rule and a type
rule, never a name rule). Density draws in graphite, camera density in the
camera kind hue, and a dropout inside a dense stream as a caution dash.
Cameras, Plots, the ruler, and the stream rows share one playhead and one
visible span. Every surface names the episode above its inspection, so the
transport leads with its controls, and Space, the arrows, and Home/End drive
the clock whether focus rests inside the inspection or on the page itself.

Data speaks the operator's language over storage's. A dataset is named
without the `dataset_` prefix every native dataset carries in SQL
(`display_name()`); the SQL name stays in the tooltip and the overview
footer. The rail names an episode by its task label or the part of its source
path that differs between files; an identifier-shaped name gives way to the
recorded time of day, and a recording whose clock started at the Unix epoch
says "no wall clock" rather than a day in 1970. Plot readouts round to the
lane's range and keep the exact value in the tooltip; a lane without values
receives a compact note, and before the first sample the readouts hold it as
cameras hold their first frame. The shared Timeline owns playback time, so
plots and cameras do not repeat sample/frame time or age. The streams ledger
prints "all" for a stream present in every episode and the deviation in
caution.

### Live

The canonical first-open Live desktop layout is one-third workbench and
two-thirds visualization, with the Timeline in the lower 42% and each camera
in its own row of the named camera column:

```text
┌──────────── one third ────────────┬────────────── two thirds ──────────────┐
│ Live workbench                    │ Scene              │ Camera tabs       │
│                                   ├────────────────────┴───────────────────┤
│                                   │ Timeline — lower 42%                   │
└───────────────────────────────────┴─────────────────────────────────────────┘
```

This is a starting composition, not a fixed semantic order. Every registered
panel may be attached to another tile as a tab or docked to an edge. Panel
identity is stable and unique; camera panels reconcile by named home when
cameras are assigned, removed, or loaded asynchronously, and content stays
keyed by panel ID so moving or tabbing a camera does not restart a feed.
Live begins with Control; recording identity and capture transport stay in
the Timeline, and recording, torque, and robot problems reach the top bar's
status line. Robot setup lives in Settings → Robots.

### Curation

Curation is a graph beside the selected node's workspace. Under the shared
document bar, the `curation-graph-v1` layout scope opens as two tiles: a 23%
Graph of 190px stage cards on the dotted bench (Dataset, then the
measurements and Selects it feeds, joined by edges that carry populations),
beside the node's Workspace. A card says what the node holds, or while it
runs what it is doing; a leaf Select wears the Output flag, and a Select over
moments says the recording it keeps ("keeps 1:53.9 of 2:26.9") because that
is the training budget. Every count below the Dataset is over the first 100
episodes the curation reads.

Every node's workspace has the same anatomy, top to bottom:

```text
┌ node bar (40px) ─ what the node reads, runs with or asks · note ── actions ┐
├ overview band (when the node has one) ─ strip / episode map / categories   ┤
├ index bar (34px) ──────────────┬ tab strip (34px, 2+ tabs) / evidence head ┤
│ rows: still · name / facts ·   │ episode inspection: cameras, Scene, and    │
│       drawing · measure        │ the timeline led by the node's own tracks  │
└────────────────────────────────┴────────────────────────────────────────────┘
```

- **Node bar:** the node's input or configuration on the left — Find
  similar's query and Search, a Select's rule as one sentence with its
  threshold read out, Break down's camera · model · sampling as the button
  that opens its settings, what Moments, Group and the Dataset read ("Every
  node reads these 100 episodes"). At the far end, in one group: a caution
  note for what needs attention (a missing setting, failed episodes), a
  quiet line for how many episodes reach the node when fewer than all do,
  with a link to the measurement they wait on ("Score the rest"), and the
  node's actions (Run N left, Retry failed, Stop, Re-score all, Regenerate)
  or a Select's tally ("Keeps 1:53.9 of 2:26.9").
- **Overview band:** Select's threshold strip — every judged value as a dot,
  the threshold as a line running into the handle of a real slider under
  them (a 4px track filled in ink on the side the rule keeps, a 16px round
  handle, its value riding the axis beneath, a 1× tick on pace axes), dots
  and handle on one track inset by half the handle so they share pixels —
  over the episode map
  ("Kept in 6 of 6 episodes"); Find similar's asked-about still, the map of
  matches and the match strip; Group's map and category chips (the open one
  lime, each with its hue as a dot).
- **Index:** one width for every node (`clamp(18rem, 36%, 28rem)`), so the
  divider holds still between nodes. Its 34px bar holds its filter, its
  Kept / Left out segments, or a caption naming its order ("6 episodes ·
  slowest first", "32 subtasks · closest first"). An index of a measurement
  lists by that measurement: Moments slowest first (failures first of all),
  Find similar closest first with a "Below 50%" divider, Group's members
  most typical first; Break down, and a Select over moments, list by episode
  under sticky headings (the open one in the selected wash) that say what the
  episode holds ("10 subtasks", "leaves out 0:09.0 of 0:16.0"), the rows
  named by their spans. Kept / Left out count rows — stretches, not seconds —
  and switching sides plays that side's first row. Rows are one roving tab
  stop.
- **Evidence:** the shared episode inspection in episode tabs (every row
  opens its episode or brings its tab forward), after a leading tab when the
  node has one (the Dataset's Overview, a category's Profile). Choosing a row
  plays from its start and keeps playing. The node's rows lead the timeline
  and the recorded streams fold behind one row.

### Workflow, Training, Rollout

Workflow's Agent view puts the graph first: one document bar, then a 24%
Assistant column beside the Graph over a Results shelf in the lower 54%;
stage details float over the graph beside the selected card rather than
holding a tile. Training is curve-first: Setup above Runs in a left rail, and
one tab group of opened runs. Rollout is a compact physical-policy bench: a
27% workbench beside a dominant live Scene, every follower camera recorded
for the policy, and one low Inference lane; the Run policy area anchors to
the bottom of a tall workbench.

### Named Rules

**The Panel-Owns-Content Rule.** If a surface has independent identity,
focus, or sizing needs, register it as a panel. Do not add another fixed
shell region or let one component mutate another tile's tabs.

**The Timeline-Is-One-Clock Rule.** Live, Data, Curation, Explore, camera
media, scene, and plots share one playhead contract. Do not create a second
timeline with different geometry or ownership.

**The Status-Reports-Exceptions Rule.** The top bar's status line reports the
connection and what needs attention. It sorts danger before caution before
plain, and when space runs out it drops the lowest-priority facts whole; it
never truncates a fact or announces a resting state.

**The Desktop-Only Rule.** Design and test at 1024 CSS pixels and wider with
mouse, touchpad, and keyboard. Do not add viewport breakpoints, mobile
navigation, touch gestures, or touch-target overrides.

**The Consequence-On-The-Page Rule.** Text describing energize, release,
stop, or discard is rendered on the page, never only in a tooltip.

**The No-Unfolding-Prose Rule.** A live value changes tint, chip, or control
state; it does not insert a sentence that pushes the panel below it.

**The Storage-Name-Stays-Backstage Rule.** Datasets are named by their
display name, without the `dataset_` prefix; the SQL name appears only where
SQL is the subject (tooltips, the overview footer, executed queries).

## Component Ownership & Data Flow

This section defines behavioral ownership, independently of Rust modules,
crates, or file structure. A **panel** is a movable spatial container. A
**component** is the owner of a user behavior, state transition, or rendering
contract. Moving a component's panel changes where it appears, never what the
component means or which behavior it owns.

### Behavioral Component Hierarchy

```text
StudioShell
├── StudioTopBar                              brand; activity nav (1–6); shortcuts;
│   ├── WorkspaceStatusLine                   connection + ranked activity facts
│   ├── NotificationCenter                    popover of messages
│   └── ShortcutSheet                         `?` dialog
├── WorkspaceFrame
│   └── PanelWorkspace                        layout mechanics only
│       ├── SemanticWorkbenchPanel
│       │   ├── LiveWorkbench                 operate / record semantics
│       │   ├── DataCatalogRail               dataset / episode selection
│       │   ├── WorkflowInspectorPanels       saved graph / node-edit semantics
│       │   ├── CurationGraph                 node cards, edges, + menus
│       │   ├── TrainingWorkbench             train / observe semantics
│       │   └── RolloutWorkbench              fixed-follower policy semantics
│       ├── PrimaryVisualizationPanel
│       │   ├── Scene
│       │   ├── DataSubject                   Overview tab + one tab per episode
│       │   ├── WorkflowCanvas
│       │   ├── CurationNodeWorkspace         node bar, index, episode tabs
│       │   ├── TrainingEvidence
│       │   └── EmptyOrLoadingState
│       ├── CameraPanel(s)
│       └── EvidencePanel
│           ├── TimelinePanel                 one shared episode-clock contract
│           ├── WorkflowResultsPanel          bounded media-aware SQL results
│           └── RolloutInferencePanel         measured policy-loop evidence
└── Toast                                     newest message, lower right
```

`SideSheet` is a shared component, not a panel: the docked inspector that
opens beside a table or grid for the one selected item. It owns the heading,
the previous / next / close controls, the keyboard (Escape closes, ↑ and ↓
step), and the entering motion; the caller owns the body. Workflow's result
table mounts it as the row inspector.

`StudioShell` owns the application frame. `StudioTopBar` chooses the active
activity (including the Settings entry) and owns the global keys 1–6, `,`,
and `?`. `PanelWorkspace` owns tabs, docking, splits, resizing,
reconciliation, and session layout state; it does not know what an episode,
camera, robot, query, or playhead means.

### Semantic Intent and Activity Projection

The workbench or rail is the **semantic surface**: it expresses what the
operator means to inspect or operate. The rest is the **visual consequence**
of that meaning. The semantic surface never reaches across the layout to
construct, replace, or mutate another panel directly.

One logical **Activity Controller** per activity sits between user intent,
backend state, and rendering. It receives activity selection from the top
bar; semantic intents from the active workbench or rail; time and transport
intents from the Timeline; typed panel-focus intents, such as choosing a
camera stream; and authoritative snapshots, telemetry, and request results
from the backend. It produces one atomic **Activity Projection**:

```text
Workbench intent ──────┐
Timeline action ───────┼──→ Activity Controller ──→ Activity Projection
Panel focus intent ────┤                                  ├── Primary visualization
Fetched backend state ─┘                                  ├── Camera panels
                                                          ├── Signal lanes
                                                          └── Timeline or inference evidence
```

An Activity Projection is a coherent description of the complete active
workspace, not a collection of independently mutable panel states. Opening
an episode in Data, for example, derives its tab, cameras, plots, Timeline
subject, and playhead together. Each activity owns independent controller
state; switching activities restores the target's selection, projection,
time/evidence subject, and panel layout without letting hidden state mutate
the visible one.

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

`Loading`, `Unavailable`, and `Failed` are real recorded-source states. They
are not aliases for Live. When a recorded sample or clip is missing, the
panel shows that recorded-source state and never substitutes current live
telemetry or a live camera frame.

Live camera panels derive from the current Live robot assignments. Recorded
camera panels derive from the selected episode's stream identities.
Activating a camera panel emits that typed live-camera or recorded-stream
identity; domain meaning is never recovered by parsing a panel ID string.

Rollout's Scene and cameras are explicit live sources scoped to the configured
follower: the Scene may show both arms for physical context, while the camera
panels mirror the follower cameras recorded for the policy. Inference
evidence is a separate measured source and stays empty until the backend has
completed a native inference.

### Timeline Interface Contract

There is one canonical Timeline component and one action vocabulary in Live,
Data, review, loading, and empty states. Unavailable actions remain part of
the interface and are disabled with a specific reason. Rollout does not own
an episode clock, so it registers Inference instead of Timeline.

The Timeline owns the visible episode clock and playhead; pointer drag and
keyboard seek; Record, Pause/Resume, Stop, and Discard capture transport;
one icon-only Play/Pause with an accessible name; Follow Live while an
episode is growing; and stream rows and explicit recording gaps.

Every Timeline action carries the identity of the recording or episode it
acts upon. Play/Pause and seeking change recorded visualizations only; they
never move a robot. Dragging or keyboard-seeking emits preview updates while
moving, then a committed seek at release. Each accepted playhead change
produces one synchronized projection for the Scene, every recorded camera,
Plots, and the Timeline itself.

### State Ownership

| State | Owner | Consumers |
|---|---|---|
| Active activity | `StudioShell` / top bar | Activity nav, active controller, status line |
| Live robot and camera selection | Live Activity Controller | Live Workbench, Scene, Live camera registry |
| Live recording and review subject | Live Activity Controller, reconciled with backend truth | Live Workbench, Timeline, status line |
| Catalog, Overview dataset, opened episode tabs, front tab | Data workspace selection | Catalog rail, subject tabs, episode inspection |
| Workflow saved graph, local draft, run snapshot, terminal result | Workflow Activity Controller, reconciled with backend truth | Node panel, canvas, Results, Assistant, status line |
| Workflow Assistant conversations and tool receipts | Backend Workflow agent, reconciled by the Workflow controller | Assistant panel |
| Curation saved graph, draft, measurements, runs, episode sample, selected node | Curation Controller (browser-owned passes, saved through the backend) | Graph, node workspace, document bar, status line |
| Curation open episode tabs, chosen row, index filter, listed side | The node workspace that shows them | Index, episode tabs, timeline tracks |
| Rollout checkpoint, execution inputs, fixed follower, readiness | Rollout Activity Controller, reconciled with backend truth | Rollout Workbench, Scene, follower cameras, Inference lane |
| Timeline subject, playhead, playback, Follow Live | Timeline Controller within the active activity | Scene, Cameras, Plots, Timeline |
| Panel geometry and active tab | `PanelWorkspace` | Panel chrome and persisted layout only |
| Hardware, recording, storage, and safety state | Backend | Activity Controller through the wire API |

### Required Interaction Flows

**Open an episode:** `Catalog rail → open(episode) → Data selection → episode
tab in front + inspection (cameras, Plots, Timeline) on one clock`. J/K moves
the front tab to the neighbour; Esc brings the dataset forward in Overview.

**Apply and run a Workflow:** `Node inspector edits → revision-checked Apply →
Workflow backend → read-only Query → refetched run/cancel state → Canvas +
bounded media-aware Results`, while browser-owned node geometry and panel
focus remain stable.

**Ask the Workflow Assistant:** `Composer → backend-owned provider turn →
relation/model inspection → read-only SQL candidate → validated graph save →
optional run → bounded query inspection → durable thread`. Saved canvas state,
run snapshots, and Results are authoritative throughout; Assistant activity
never creates a browser-local graph or a second Run control.

**Seek or rewind:** `Timeline → Seek(subject, time, phase) → Activity
Controller → one synchronized recorded projection`.

**Receive live telemetry:** `Backend event → Live Activity Controller → Live
projection → Scene + Cameras + Signals + Timeline`.

**Select a rollout checkpoint:** `RolloutWorkbench → SelectCheckpoint(id) →
Rollout Activity Controller → fixed follower + live Scene + follower cameras
+ truthful Inference state`. Run remains disabled until the checkpoint,
follower, camera binding, calibration, and fresh control stream are
available; the backend revalidates the complete contract before motion.

### Named Rules

**The Semantic-to-Projection Rule.** Workbenches emit intent; they do not
mutate visualization components. One controller derives the complete
visible consequence.

**The Atomic-Projection Rule.** Primary visualization, cameras, signals, and
Timeline are updated from one coherent activity projection.

**The Explicit-Source Rule.** A visualization receives Live or Recorded as an
explicit source with explicit loading and failure states. Missing recorded
data never falls back to Live.

**The No-Ambient-Identity Rule.** An action names the episode, stream, robot,
or query it affects. A component never guesses its target from another
activity's selection or from a panel ID naming convention.

**The Timeline-Owns-Time Rule.** Timeline owns the user-facing time
operation; the Activity Controller owns the synchronized projection; the
backend owns any physical consequence.

## Elevation & Depth

Studio is flat at rest. Hierarchy comes from the canvas / sidebar / paper
tonal steps, hairlines, and ink edges. Shadows are soft and blurred, each
paired with a faint 1px ring so the edge stays crisp on paper, and they
appear only on things that float above the workspace or stand on the graph
bench.

### Shadow Vocabulary

- **Raised** (`box-shadow: 0 1px 2px rgb(16 16 16 / 0.08), 0 0 0 1px rgb(16 16 16 / 0.05)`):
  The checked cell of a segment group, lifted out of its recessed track.
- **Float** (`box-shadow: 0 4px 14px rgb(16 16 16 / 0.08), 0 0 0 1px rgb(16 16 16 / 0.04)`):
  A stage card under the pointer (with a 1px rise); the workbench crate's
  own floating chrome. Stage cards rest one step lighter
  (`0 1px 2px rgb(16 16 16 / 0.05)`) because they stand on the bench.
- **Popover** (`box-shadow: 0 12px 32px rgb(16 16 16 / 0.14), 0 0 0 1px rgb(16 16 16 / 0.06)`):
  The notification center, the document switcher, the stage details sheet,
  the dock compass, and the toast.
- **Dialog** (`box-shadow: 0 24px 64px rgb(16 16 16 / 0.22), 0 0 0 1px rgb(16 16 16 / 0.06)`):
  Blocking confirmations and the shortcut sheet, over a 28% ink backdrop.
- **Selection halo** (`0 0 0 3px` lime at 45%, with an accent-strong hairline):
  The selected stage card and contact-sheet tile. This is state, not height.

### Named Rules

**The Flat-at-Rest Rule.** Resting panels, tabs, rails, ledgers, strips,
fields, and buttons cast no shadow. Only floats (popovers, dialogs, toasts)
and objects on the graph bench carry one.

**The Pressed-In Rule.** A toggle that is on is pressed into the page: the
recess fill, an inner hairline, ink text at weight 560. A segment group
inverts it (a recessed track with the chosen cell lifted as raised paper).
State changes never add geometry.

## Shapes

A gently rounded ladder where the radius grows with the object: 4px for
chips, kbd keys, kind marks, and tree expanders; 5px for tree rows, Settings
nav items, segment cells, and stage glyph marks; 6px for buttons, fields,
selects, fact tile sets, and the dock preview; 8px for cards (stage cards,
contact-sheet tiles, the streams and datasets ledgers), mode groups, the
toast, the empty-state glyph tile, the compass, and the 30px overview kind
mark; 10px for dialogs and the larger floating sheets (notification center,
document switcher, stage details, import review). Full circles are for status
dots, the notification count, the transport's play button, and slider
thumbs. Panel and tile containers stay square so adjacent surfaces meet as
one workspace.

Icons are outline geometry at a 1.4 stroke with round caps and joins, 14–16px
inside a 26–30px target. Kind marks are one mono letter in a 1.5px outline of
their hue; stage marks put the stage glyph in the same outline. Tab and nav
indicators are 2px ink bars with 2px top corners.

## Components

### Buttons

Every interactive control is a `Button`, `LinkButton`, `FileButton`,
`ModeButton`, or `Toggle` from `spiral-robotics-ui`. Product code declares the
verb first; visual presentation never changes the verb.

- **Shape:** 6px radius, 12px medium label, 8px horizontal padding; compact
  26px, standard 30px (10px padding), prominent 34px (12px padding, 13px).
  `ui-button--square` makes any variant a square; `ui-icon--spin` turns the
  spinner once every 900ms and is the only self-moving icon.
- **Primary (Solid):** Ink fill, white 560 label, hover one step lighter
  (`{colors.primary-hover}`), once per local purpose: Run, Run training,
  Run policy. A kbd chip may name its
  shortcut inside it (Run ⌘↵), drawn translucent white on ink.
- **Outline:** Paper on a hairline, soft ink; hover firms the hairline to
  line-strong and the label to ink; press steps to the recess.
- **Quiet / Icon / Link / Row / Tab:** Quiet takes the hover wash; icon
  buttons are faint ink squares; links are soft ink that underline in
  line-strong on hover; tabs are faint until current, then ink at 560 with no
  fill (the strip draws the indicator).
- **Pressed (toggle on):** see the Pressed-In Rule. **Selected (list item):**
  the lime `selected` wash with accent-ink text at 560, never a ring.
  **Expanded:** ink on the hover wash; the chevron already turned.
- **Danger:** Delete, Discard, Replace, Release, and Stop use the shared red
  treatment. Their consequence determines the confirmation level.
- **Focus:** A 2px blue outline at 1px offset, always visible inside dense
  chrome. A confirmation focuses its safe action, Escape cancels, and
  dismissal returns focus to the originating action.
- **Pending:** The initiating action stays in place, names the work in
  progress, exposes `aria-busy`, and disables repeat submission.

#### Action grammar

- **Create** makes a durable entity; **Assign** creates a relationship.
- **Connect** and **Disconnect** change a live device connection.
- **Remove** or **Unassign** detaches a reversible relationship.
- **Close** dismisses a view while preserving its data.
- **Delete** permanently removes a durable entity.
- **Discard** abandons uncommitted work; **Replace** overwrites a scope.
- **Apply** commits configuration or calibration; **Release** turns off motor
  torque and always states the physical consequence.
- **Sync** reconciles clocks or state; **Retry** repeats failed work. Start,
  Stop, Cancel, Import, Export, Refresh, Navigate, Select, Toggle, Hold, and
  Teleoperate retain their literal meanings.

#### Placement and confirmation

- View controls live in the panel or dialog header; collection-wide actions
  in the collection toolbar; row-local actions at the trailing edge; Save and
  Cancel in the editor footer; physical-safety actions in the safety region.
- Recoverable actions happen immediately. Local permanent actions use the
  shared inline confirmation. Scope-wide replacement and physical
  consequences use the shared blocking dialog (10px, dialog shadow, a danger
  consequence box on its wash and line, a sidebar-toned footer). Native
  browser confirmation is not part of the language.

### Chips and Keys

- **Chip:** One word of state in 11px mono, 20px tall, 4px radius, on its
  tinted wash behind a hairline of the same hue (accent/good, caution,
  danger); neutral chips are paper on a hairline. A chip renders only while
  its condition matters.
- **Kbd:** A shortcut named beside the action it takes: 18px, 4px radius,
  10px mono, faint ink on the recess behind a line-strong edge. It appears in
  the rail filter (`/`), inside a solid action (⌘↵), beside step hints (J K),
  and in the shortcut sheet.
- **Kind mark:** `KindMark` — one mono letter (D dataset, E episode) in a
  1.5px outline of its kind hue, 16px, 4px radius; 14px in tab strips and the
  subject path (inactive tabs at 70% opacity), 30px/8px beside a subject
  title.

### Top Bar and Navigation

- **Bar:** 44px paper, hairline below, three grid columns (lead, status line,
  trail); the trail is separated by its own hairline.
- **Nav item:** The full bar height, a 15px glyph and 13px word, faint ink at
  450; hover goes to ink with no fill. The current activity is ink at 560
  with a 2px ink underline inset 8px, which grows in from 40% width over
  220ms on the expo ease.
- **Status line:** 11px mono, faint ink. The connection dot is good-dot at
  rest, caution for loading or setup problems, danger and pulsing while
  reconnecting. Caution facts are rust; danger facts are red at 560.
- **Trail actions:** 30px icon buttons (16px glyphs); the one whose surface is
  open reads pressed (ink on the recess). The notification count is a lime
  pill with a paper ring, 9px mono.
- **Shortcut sheet:** A 40rem dialog grouping keys by where they work
  (Anywhere, Data, Episode playback, Workflow and Curation, Live).

### Panel Tabs, Tiles, and Splits

- **Tab bar:** 34px on the sidebar tone. Tabs are 12px with 11px padding and
  a leading icon or kind mark; the current tab is paper, ink at 560, with a
  2px ink bar along its lower edge inset 10px.
- **Single-tab strip:** A tile with one tab draws no indicator at all: one
  tab is a header, not a choice.
- **Splitter:** A 1px hairline with an 8px hit area; focus uses the blue
  outline. Double activation restores 50/50; arrows resize, Shift for a
  larger step.
- **Dock preview:** Five zones per tile; the chosen zone is lime at 22%
  behind an accent-strong edge, 6px radius. The compass is paper, 8px, with
  the popover shadow.
- **Keyboard tabs:** Left/Right, Home, End change the active tab; Alt+Shift
  plus arrows splits/docks; Alt+Shift plus Page Up/Down moves between tiles.
- **Persistence:** Layout mutations store immediately; pointer resizing
  stores on release. Invalid saved trees fall back to the canonical layout.

### Tree (Catalog Rail)

`Tree` and `TreeItem` are the nested list the catalog rail draws. ↑ ↓ Home
End move between visible rows; → and ← open and close a branch or step to its
parent.

- **Row:** 28px minimum, 5px radius, inset 6px from the rail edges, soft ink;
  hover washes it and steps the label to ink. A row is one surface: nothing
  inside it draws its own box.
- **Selected:** Solid lime fill with ink text; its counts and mono facts turn
  accent-ink and its length bar turns ink. Hover darkens the lime 12% toward
  ink.
- **Focus:** A 2px blue ring inside the row so a focused row never overflows
  the rail.
- **Motion:** Rows of an opened branch deal in over 220ms on the expo ease,
  12ms apart, capped at 84ms; the expander chevron turns over 160ms.

### Instrument Strips and Ledgers

- **Stat strip:** `FactList` in its strip layout: equal columns between soft
  hairlines, a 10.5px upper-case label over a mono value, divided by
  hairlines. Fact tiles use the same label over value inside a 6px hairline
  grid.
- **Ledger:** An 8px hairline card holding a table with a sticky recessed
  header row of 10.5px upper-case mono labels; 12.5px cells with 7px/12px
  padding on soft hairlines; numbers right-aligned in soft-ink mono; rows
  wash to the sidebar tone on hover. The datasets home and the streams ledger
  share it; the streams ledger leads with cameras (camera glyph in its kind
  hue) and prints schema names in faint mono.
- **Distribution:** The episode-length histogram: 104px of bars at ink 20%
  with 2px top corners and 3px gaps, a hovered bar in solid ink, a dashed
  faint-ink average line with a 4px-radius mono tag, and a 10.5px mono axis.
  Bars rise from the baseline over 520ms, 9ms apart, when a dataset opens.

### Inputs / Fields

- **Components:** `TextInput` (a `TextInputKind` picks text, search,
  password, number, decimal, integer) and `Select`, wrapped by `Field` when a
  label names them; `FieldLayout::Inline` puts the label beside the control.
- **Style:** 26px minimum, paper, 13px ink, 6px radius, one hairline, 8px
  padding; hover firms the hairline to line-strong. Numeric kinds are 12px
  mono tabular. Placeholders are faint ink.
- **Focus:** Border turns blue with a 3px blue halo at 18%.
- **Invalid / Disabled:** Danger border; disabled fields sit on the recess in
  faint ink.

### Side Sheet

- **Geometry:** 320px, at most 45% of its tile, a left hairline, paper, its
  own scroll. It takes width from the table or grid it inspects; it never
  overlays.
- **Heading:** Sticky, a tabular title ("Row N", "Episode N") with a faint
  "of M", then the `↑ ↓ ×` actions. Focus lands on the title when it opens.
- **Keyboard:** Escape closes and returns focus to the invoking tile or row;
  ↑ and ↓ step to the neighbour.
- **Motion:** Enters over 220ms on the expo ease from 8px right; a 120ms fade
  under reduced motion.

### Popovers, Dialogs, Notifications, and Toasts

- **Popover:** 320px paper sheets at 10px with the popover shadow, entering
  over 180ms (document switcher, stage details) or 220ms (notification
  center, 24rem, from 6px above at 98% scale, anchored top right).
- **Dialog:** 10px, dialog shadow, entering over 200–220ms from 6px below at
  98.5% scale over a 160ms backdrop fade.
- **Toast:** The newest message at the lower right: paper, 8px, popover
  shadow, 12px soft ink with a 7px severity dot (good-dot, caution, danger),
  entering over 320ms. Pressing it opens the notification center. Every
  message carries its own expiry; failures read longer.
- **Notice:** Inside a panel, `Notice` is one line of status on its tone's
  wash; danger is the only tone announced as an alert. `EmptyState` is an
  optional glyph in an 8px paper tile, one 13px title, and a faint sentence
  or two.

### Workflow Graph and Document Bar

- **Document bar:** Shared by Workflow and Curation: the glyph, the Explore |
  Agent segments (Workflow), the name as a quiet button with a turning
  chevron that opens the 320px switcher popover, then at right the state chip
  (Saved · good dot, Unsaved changes · caution, Changed in another session ·
  danger), Discard while dirty, outlined Save, and the solid Run with its ⌘↵
  kbd chip. A longer state grows leftwards without moving Save or Run.
- **Stage card:** 224×92 paper, hairline, 8px radius, resting bench lift: an
  18px outlined stage mark beside a semibold title, a two-line summary, and a
  footer naming the kind in upper-case in its tint, then the native function
  or folded operations in mono. Hover firms the hairline and lifts 1px with
  the float shadow; selection is the lime halo. Cards enter left to right,
  320ms each and 45ms apart.
- **Graph canvas:** The pale bench with a dot grid; zoom in, zoom out, and fit
  share one hairline group at bottom left. While a run is admitted every edge
  turns blue and carries marching dashes; run and tool dots pulse blue.
- **Stage details:** On demand, never a tile: one 320px popover beside the
  selected card, following it through pan and zoom; Escape or deselection
  removes it.
- **Results:** One run strip (phase dot, run selector only when more than one
  run exists, mono row count and elapsed), then the shared result table:
  image previews (88×50 on the camera mat) lead, repeated strings become
  category chips, varying numbers carry a bar between the column's extremes,
  timestamps read as UTC clock time, identifiers step back into faint mono.
  The row inspector is the shared side sheet.
- **Assistant:** The thread and composer share the tall column; the composer
  is one bordered field with a square send at bottom right and a blue focus
  border and halo. The placeholder carries the Assistant's state.

**The Display-Only-Inference Rule.** A cell's kind (image, chip, bar, clock
time, identity) changes how a stored value is drawn, never what it is. The
exact value stays in the tooltip and the inspector, and the presentation
never invents a count, class, or total.

### Curation Index Rows

**Every row leads with a still.** An index is scanned before it is read, so
every row of every node — a subtask, a whole episode, a stretch of one — is
one anatomy at one height: a 96×54 still on the camera mat, the name over a
mono facts line, and the measure in a fixed right-hand column.

- **Still:** a subtask's middle as the annotation pass stored it; an
  episode's or a stretch's middle cut on demand on the camera its node read
  (else the configured camera, else the overview camera), one query per
  episode, cached per camera, episode and instant for the whole curation.
  Waiting is a shimmer on the recess; failure says why on the mat.
- **Name:** a subtask's words, clamped to two lines; a recording's label
  when its path gives one; otherwise its id tail ("…206c08ff") in mono.
- **Facts line:** 11px mono, with one small drawing before the facts: an
  episode's length against the longest listed (square-root scale, as Data's
  rail), where a stretch sits in its episode, or an episode's pace across
  its recording against a dashed 1× line.
- **Measure:** whole percentages for probabilities ("97%"), pace as "0.90×",
  a recorded fact in its own unit. Its tooltip says what the number is.
- **Chosen:** the selected wash; its place bar turns ink. One row is a tab
  stop — the chosen one, else the first — and the arrows move between rows.
- **Folded:** what an index counts but does not list says so once after its
  last row ("94 episodes not scored yet"), never as a row per item.

**The Measured-Order Rule.** An index of a measurement lists by that
measurement, and its bar says so. Recording order is for the Dataset node
and for indexes grouped by episode.

**The Said-Once Rule.** A fact has one home: what a node holds is on its
card, what it runs with and what reaches it on its bar, what a row is in its
row, and the playing span's words in the timeline's readout.

### Timeline and Plots

- **Transport:** One toolbar: jump, step, the round ink Play/Pause, the 16px
  mono elapsed clock with the total in faint 11.5px mono, speed, Loop, and
  zoom (`+ − 0`, Ctrl/⌘-wheel). Record, Pause/Resume, Stop/Discard join it in
  Live; Record always implies Teleop.
- **Rows:** A filter field, a sticky mono ruler, then one row per stream:
  expander, show checkbox (mixed when only some fields are plotted), kind
  glyph, mono name, mono message count, and a density track. The playhead
  crosses ruler and rows.
- **Plots:** One section per plotted stream, one lane per field with a
  labelled value axis and one trace per array index; the readout column
  doubles as the legend (swatch, index, value at the playhead or cursor).
- **Evidence tracks:** A surface's own rows lead the stream rows: a value
  track (Curation's pace, 168px tall, read out as "0.72×" against a 1×
  expert-pace reference line, with a Select's cutoff dashed and the
  stretches it leaves out in a graphite wash — a decision, not a fault), or
  a span track (subtasks). Spans are paper chips with a graphite edge; the
  one under the playhead takes an ink edge and only the chosen one is lime;
  a grouping lends each its category's hue. A chip too narrow for a word
  shows none, and the track's name column reads out the span under the
  playhead in full. Activating a span plays from its start and keeps
  playing. Under evidence the recorded streams wait behind one "N streams"
  row.
- **Episode stepper:** Wherever a surface walks a list of episodes it uses
  ↑ and ↓ icon buttons that also answer the arrow keys.

### Camera Panels

- **Frame:** Dark mat inside light panel chrome, contained pixels, honest
  letterboxing. Loading, decoding, and failure are told on the same mat in
  camera ink, so a camera never flashes from paper to black.
- **Lifecycle:** One stable MJPEG element per assigned Live camera; recorded
  H.264 frames decode through native WebCodecs and seek from the preceding
  keyframe. Moving or tabbing a panel never fabricates a second connection.
- **Failure:** Actionable unavailable or permission text appears in the
  frame; a green dot never masks a blank image.

### Workbench Sections, Training, Rollout, Settings

- **Workbench sections:** A 10.5px upper-case faint label names each control
  group (Live, Training, Rollout, device setup); the section's content is the
  heading's only subordinate.
- **Robot control:** The Control section exclusively owns Hold, Release, and
  Teleop as a mode group; Hold and Teleop read in their checked state,
  Release in the danger treatment, with the inline Release confirmation.
  Setup lives in Settings → Robots.
- **Training:** A curve-first run desk. Setup keeps its one solid Run
  training beneath the scrolling form; Runs is a newest-first ledger; every
  opened run is a sibling tab with its loss curve, data path, throughput,
  events, and logs keyed to optimizer step. No synthetic telemetry.
- **Rollout:** Checkpoint picker, one compact fixed-follower line, action
  steps and control rate, and the single solid Run policy with one specific
  blocker beside it. The Inference lane reads Latency, Model, and Misses as
  label-left, mono-value-right metrics that stay em dashes until the backend
  measures them.
- **Settings:** Replaces the activity workspace while the top bar stays: a
  sidebar-toned section nav (5px rows; the current section is solid lime,
  ink at 560) beside ordinary forms. Robots is the default, then SpiralDB,
  Training, and Video. Restore alone uses the blocking dialog.

**The Fixed-Follower Rule.** Rollout never asks the operator to choose a
motion target. It commands only the configured follower, and backend
readiness decides whether execution may begin.

**The Shared-Optimizer-Step Rule.** Any training view that plots, marks, or
summarizes run progress keys that evidence to optimizer step.

### Motion

Everything that enters moves on one easing, `--ease-out-expo`
(`cubic-bezier(0.16, 1, 0.3, 1)`). Colour and hover changes take 120ms
ease-out. Entrances: popovers 180–220ms, dialogs 200–220ms, side sheet 220ms,
subject panels 260ms with a 4px rise, toast 320ms, stage cards 320ms. Lists
deal in with short staggers (tree rows 12ms, dataset rows 40ms, tiles 12ms,
stage cards 45ms) and the length histogram rises over 520ms, 9ms per bar.
Only work in progress loops: the 900ms pulse on run and tool dots, the
reconnecting dot's 1.2s pulse, the spinner, and the 1.4s loading shimmer.
Under `prefers-reduced-motion`, dialogs and the side sheet keep a 120ms fade
and every rise, stagger, bar growth, and loop is removed.

**The One-Easing Rule.** Entrances use `--ease-out-expo`; loops exist only
while work is in progress; reduced motion keeps fades and nothing else.

## Do's and Don'ts

### Do:

- **Do** spend lime only on the current selection: the selected tree row,
  Settings section, list item, stage card, or tile.
- **Do** mark the current activity and the current tab of a multi-tab strip
  with a 2px ink underline, and leave a single-tab strip without any
  indicator.
- **Do** fill exactly one primary action per region in ink, with its kbd chip
  when it has a shortcut (Run ⌘↵).
- **Do** use the 2px blue outline for keyboard focus everywhere, and blue for
  links and running work.
- **Do** draw semantic state as its ink on its soft wash behind its own line
  colour.
- **Do** set every measurement and identifier in Geist Mono with tabular
  figures, and put small upper-case labels over mono values in hairline
  strips.
- **Do** keep body text at 13px, subject titles at 19px, and the elapsed
  clock at 16px mono as the largest thing inside a panel.
- **Do** read toggles as pressed-in neutral cells (recess, inner hairline,
  ink at 560), and segment choices as raised paper in a recessed track.
- **Do** keep the status line to the connection plus exceptions, sorted
  danger, caution, plain, and let narrow widths drop the lowest facts whole.
- **Do** name datasets without the `dataset_` prefix and keep the SQL name in
  the tooltip and footer.
- **Do** lead catalog rows and tabs with a kind mark (D, E) and workflow
  stages with an outlined stage mark.
- **Do** follow the radius ladder: 4 chips/kbd, 5 rows/segments, 6
  buttons/fields, 8 cards/strips/toast, 10 dialogs and floating sheets.
- **Do** open episodes as tabs beside Overview, step the front tab with J/K,
  and return with Esc.
- **Do** register independent surfaces as panels with durable IDs, and keep
  every activity's layout state independent.
- **Do** give every drag, dock, split, resize, and tab action a keyboard path,
  visible focus, and reduced-motion behavior.
- **Do** keep one Timeline action vocabulary and include the recording or
  episode identity in every time or transport action.
- **Do** route workbench, Timeline, panel-focus, and backend events through
  one activity controller that derives the complete visible projection.
- **Do** keep Rollout fixed to the configured follower, and keep its metrics
  empty until native measurements arrive.
- **Do** spell every colour once as a `--tone-*` role token and read it
  through `--ui-*`, `--wb-*`, or `--chart-*`.
- **Do** remove a component, property, selector, token, or dependency feature
  when its last real consumer disappears.

### Don't:

- **Don't** restore the left activity rail, the bottom status bar, the 12px
  system font, or blue selection.
- **Don't** use lime for focus, links, primary actions, tab indicators,
  headings, or decoration.
- **Don't** fill a primary action with a hue; the filled action is ink.
- **Don't** mark selection with a ring on buttons or list items; the ring is
  focus's.
- **Don't** put upper-case tracked labels above headings as kickers or
  eyebrows; upper-case names a value, column, or control group.
- **Don't** set a measurement or identifier in the proportional face, or a
  sentence in mono.
- **Don't** draw an indicator on a strip that holds one tab.
- **Don't** truncate status facts or announce a resting state in the status
  line.
- **Don't** show the `dataset_` storage prefix in a dataset's display name.
- **Don't** cast shadows from resting panels, rails, ledgers, strips, fields,
  or buttons, or use a hard zero-blur offset shadow.
- **Don't** add a second easing curve or a loop that outlives the work it
  reports.
- **Don't** add fixed one-off shell regions; take a generic mechanic upstream
  to `dioxus-workbench` and register domain content as a panel.
- **Don't** let a Data action mutate Live camera tabs, or derive recorded
  camera tabs from current hardware assignment.
- **Don't** treat missing recorded data as permission to show live telemetry
  or a live camera inside a recorded episode.
- **Don't** infer an action target from ambient selection or by parsing a
  panel ID.
- **Don't** create a second Timeline, or let scrub move only the playhead.
- **Don't** unfold live prose, or encode episode quality in card size.
- **Don't** let a stage or category hue stand alone without a word, glyph, or
  mark carrying the same meaning.
- **Don't** fabricate rollout readiness, latency, or training telemetry.
- **Don't** introduce viewport breakpoints or alternate mobile/touch UI.
- **Don't** hide what a Select leaves out: its Left out side lists it with
  its measure, and the pace track washes it in graphite, not danger.
- **Don't** list one row per item a node has not measured; count them once
  after the last row.
