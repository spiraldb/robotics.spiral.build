---
name: rerobot studio
description: A compact, one-light control bench for connecting arms, recording episodes, replaying them, and exporting datasets.
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
  arm-1: "#4a90d9"
  arm-2: "#9a6dd7"
  arm-3: "#4dab9a"
  arm-4: "#e0872f"
  axis-x: "#b65b55"
  axis-y: "#49815f"
  axis-z: "#5178a6"
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
  notice-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "4px 8px"
---

# Design System: rerobot studio

## Overview

**Creative North Star: "The One-Light Control Bench"**

rerobot studio is one calm, literal workspace: white panels surround a pale
WebGL bench, and the dataset tools continue the same light material across the
foot of the shell. Graphite ink, hairline separators, small radii, and tabular
data make it feel like a precise bench utility rather than a dashboard or a
diagnostic console. Camera pixels alone keep a dark mat for honest letterboxing.

The interaction story is equally direct: connect arms, teleoperate, Record,
select an episode, Replay it on a compatible arm, then Export the completed
episodes. Measurement and fault data remain available where they help operate
the hardware, but judgement, traces, thresholds, and diagnostic visualization
do not become competing primary lanes.

Density is the point, not a side effect. The base face is 12px, ordinary
controls are 24px tall — the WCAG 2.5.8 target floor, used as a floor rather
than beaten — and most gaps come from a 2/4/6/8px rhythm. Record is the one
control held at 28px, because it is reached for with a hand still on the arm.
The 16px elapsed readout is the largest type in the app and sits on Record's
own baseline rather than stacked above a caption. Colour is spent on selection,
safety, connection, and arm identity; structure comes from alignment and
one-pixel rules.

Density only reads as precision if the surface holds still. Telemetry arrives
twenty times a second, so every live number sits in a box wide enough for its
widest value, status reports the exception rather than the resting state, and
no explanatory sentence unfolds and collapses as the hardware changes. A dense
instrument that twitches reads as noise; a dense instrument that stays put
reads as a bench tool.

**Key Characteristics:**

- One light neutral world across shell, panels, WebGL stage, and dataset rail.
- A persistent three-pane studio with a compact global session region below it.
- One literal dataset loop: Record → select an episode → Replay → Export.
- Compact 12px type, 24px controls, small radii, and hairline separators.
- Monospace tabular figures for elapsed time, rates, durations, counts, and IDs.
- Live values in fixed boxes and status that reports only the exception, so the
  page never twitches under a telemetry stream.
- Role blue for local primary actions; semantic colour only for real state.
- Equal-size episode cards favor scanability over proportional visualization.
- Responsive reflow keeps recording state, actions, blockers, and safety visible.

## Colors

The palette is warm-neutral paper and graphite, with a quiet role blue and a
small set of semantic and identity accents.

### Primary

- **Role Blue** (`{colors.accent}`): Selection, pressed pane controls, Record,
  Replay, Connect, and the single filled action a region exists to support. Its
  wash (`{colors.selected}`) marks selection without flooding the workspace.

### Secondary

- **Brick Red** (`{colors.danger}`): Destructive actions, recording-in-progress,
  faults, energized torque, and incomplete episodes. The soft wash carries live
  or problem state.
- **Bench Ochre** (`{colors.caution}`): The exceptional operating conditions —
  simulation, an environment that cannot be verified, a degraded control rate,
  a lagging follow error, and an arm that still needs calibrating. Physical
  hardware is the ordinary case and takes no wash at all.
- **Connection Green** (`{colors.good}`): A 6px connection or completed-state
  dot; it does not fill whole controls.

### Tertiary

- **Arm identity accents** (`{colors.arm-1}` through `{colors.arm-4}`): Bind a
  robot row, viewport legend entry, and rendered arm into one object. The usual
  leader/follower pair uses blue and violet; orange remains last because the
  gripper jaws already use orange.
- **Axis accents** (`{colors.axis-x}`, `{colors.axis-y}`, `{colors.axis-z}`): The
  small WebGL orientation key only. They are deeper than the lines drawn in the
  scene because they are 10px *text* on the pale bench and have to clear 4.5:1;
  the vivid originals measured 2.0–2.8:1.

### Neutral

- **Paper** (`{colors.canvas}` / `{colors.surface}`): The shell, panels, rail,
  cards, fields, and floating notices.
- **Pale Bench** (`{colors.viewport}`): The WebGL workspace. It is a light neutral
  step below paper so pale robot shells remain legible without creating a second
  visual world.
- **Camera Mat** (`{colors.camera}`): Camera frames and unavailable-camera
  placeholders only. It belongs to image content, not application chrome.
- **Recess** (`{colors.sunken}`): Setup groups, readout cells, disabled filled
  controls, and the mobile surface switcher trough.
- **Hairline** (`{colors.line}`): Resting borders, dividers, and slider tracks.
- **Graphite ramp** (`{colors.ink}` → `{colors.ink-soft}` →
  `{colors.ink-faint}`): Primary labels, secondary text, then metadata.
- **Hover wash** (`{colors.hover}`): The default hover treatment for quiet
  controls and rows.
- **Lift** (`{colors.lift}`): The one shadow colour the studio names for itself,
  used by the mobile switcher's active tile. Every other shadow comes from the
  framework's own small and large steps.

### Named Rules

**The One-Light-World Rule.** Application chrome, the 3D stage, and the dataset
rail all belong to one light workspace. Do not reintroduce a fixed-dark viewport
or dark session rail; only camera pixels use the camera mat.

**The Spent-Colour Rule.** Colour means selection, arm identity, connection,
caution, danger, or a primary action. If none applies, use graphite, a tonal
step, or a hairline.

**The Exceptional-Status Rule.** A chip reports an exception; it never announces
the resting state. Simulation gets a wash, physical hardware gets a plain faint
word. A mode chip exists while a mode is running or faulted, not while it is
stopped. A torque count exists while an arm is energized, not while every arm is
released. A badge that is always present is a badge nobody reads, which is
exactly the badge you cannot afford on the row that reports live motors.

**The Dimmest-Surface Rule.** A text colour is judged against the darkest
surface it can land on, not the lightest. Faint ink appears on paper, on recess,
and on the pale bench, and danger appears on its own soft wash — so each is
chosen to clear 4.5:1 on the worst of those, never on white alone.

**The One-Spelling Rule.** Reusable interface colours are declared as
role-named `--tone-*` properties in `crates/rerobot-studio/tailwind.css`.
Components refer to roles, never hue names or one-off theme variants. This
covers shadow colour too: a `box-shadow` in a component rule names a token
rather than writing its own alpha-black. Renderer-only arm and grid colours
remain centralized in `viewport.rs` and `scene.rs`.

## Typography

**Display Font:** None; this is an operating surface, not a presentation.

**Body Font:** System UI (`-apple-system`, BlinkMacSystemFont, SF Pro Text,
Segoe UI, sans-serif).

**Label/Mono Font:** `ui-monospace`, SF Mono, JetBrains Mono, Menlo, monospace.

**Character:** The system face is quiet and platform-native. Monospace is
functional: it stabilizes changing values and makes IDs and rates scan as data.

### Hierarchy

Four steps, and four is the whole list. A fifth size is drift, not a decision.

- **Readout** (500, 16px mono, 1): Elapsed recording time beside Record, and
  nothing else in the studio.
- **Title** (600, 12px, -0.01em): Wordmark, panel titles, and robot identity.
- **Body** (400, 12px): Buttons, fields, mode controls, sentences, and ordinary
  values. **Action** is the same 12px at weight 500 — the Record transport — so
  emphasis costs weight rather than a size step.
- **Label** (400–500, 11px): Status, section names, blockers, control labels in
  the viewport, joint names, and supporting copy. Weight distinguishes headings
  without adding a new size.
- **Micro** (400, 10px): IDs, durations, counts, rates, file paths, metadata,
  and compact device facts. Functional 10px copy is reserved for dense
  supporting data, never the only label on an action, never a blocker, and
  never the description of a physical consequence.

### Named Rules

**The Tabular-Numbers Rule.** Any number that changes in place—elapsed time,
Hz, degrees, duration, frames, counts—uses monospace with tabular figures.

**The Still-Numbers Rule.** Tabular figures equalize digit *widths*; they do not
equalize digit *counts*. Any value redrawn from telemetry also gets a box wide
enough for its widest reading, so nothing beside it moves when a hundreds digit
drops: the elapsed timer holds 56px, the control rate 64px and right-aligned,
replay progress 96px, a joint angle 44px, and a readout's digits 3.5ch so its
unit never slides. A number that walks its neighbours across the screen is a
worse instrument than a number one character narrower.

**The 16px-Ceiling Rule.** The elapsed timer is the studio's visual maximum at
16px. Do not restore the former 42px or 22px timer, and do not introduce display
typography. The rail is instrumentation, not a scoreboard.

**The Four-Sizes Rule.** 16 / 12 / 11 / 10, and every one of them has a job
above. A new element picks the tier whose job it is doing; it does not
introduce a size because it wants to feel slightly different from its
neighbour.

**The Sentence-Case Rule.** Labels are sentence case. Do not add uppercase
tracked kickers, eyebrows, or decorative display faces.

## Layout

The first viewport is a full-height shell with a 28px minimum header, a
three-pane body, and the dataset rail spanning the entire width below the panes.
Header and rail together cost about 135px, so the workspace keeps the rest: the
studio is dense because the instrumentation is thin, not because the content is
crowded.
The left robot explorer defaults to 272px and resizes from 220–380px; the right
inspector defaults to 320px and resizes from 280–440px. The pale WebGL bench
absorbs the remaining width. Pane toggles and the rail toggle live in the
header; the rail remains global rather than belonging to the viewport pane.

The header is a shallow operating strip: wordmark and pane controls, the
teleoperation pair, then a right-aligned status cluster. Teleoperation is the
only mode the header configures, because it is the only one that is about a pair
of arms and so belongs to no single panel; manual control names one arm and
lives on that arm's joint panel, next to the sliders it enables. The mode bar
wraps rather than scrolling, and below 1280px it drops to a full-width row of
its own; the strip is shallow because it fits, never because something was
sheared off its right edge; the teleoperation consequence stays on one
unbreakable line, so what wraps is the whole phrase rather than the middle of
it. At rest the status cluster is a faint environment word, the control rate,
and the connection state — three items, no badges. The mode narration takes a
whole line, and only for a replay: every other mode is legible from the control
that set it, so the strip stays one row through an ordinary session. Warnings
show the first issue's own words beside the count, with the rest in an anchored
details popover — never a full-width banner between the operator and the
workspace.

A robot entry is one 24px line reading identity → role → readiness →
connection, with the role giving way first when the panel narrows. Expanded
setup is three sections and a file row: motor bus — which owns the port picker,
Rescan, Identify, and the Connect action on the port's own line — then cameras,
then calibration, then file maintenance. Connection state is not restated inside
the setup; the entry line above already carries it as readiness text and a dot,
and the Connect button's own label carries the rest. Adding an arm and importing
one share a single quiet row at the foot of the list rather than stacking two
full-width bars that read as two more robots.

The joint inspector reads identity and mode controls, four readout cells, the
joint stack, then cameras. Camera previews are one per row at the panel's full
width: two side by side in a 320px inspector gave each a 150px picture, which is
too small to answer the question a preview exists for.

The dataset rail has two horizontal layers and no heading rows. The first is a
single row holding Record and the 16px timer on its baseline, then the episode
strip as an elastic middle, then session count, Import, and Export. The strip
scrolls horizontally through equal 72×36px episode buttons in capture order,
each carrying three facts on two lines: id and duration share a baseline,
exportable state sits under them. When the cap hides older takes, a dashed card
at the head of the strip says how many. The second 28px-minimum row names the
selected episode, lists duration, frames, rate, and source mode, then Replay —
replaced by frame progress and Stop while that episode is the one driving an
arm. Delete belongs to the card and is revealed on hover or focus, so the card's
own labels keep the full 72px rather than budgeting a permanent gutter around
them. Card width never encodes duration; values are printed explicitly. The
selected episode defaults to the newest completed episode when the rail first
opens.

At 900px and below, the header and dataset detail may wrap; the viewport spans
the upper 56% of a two-column body and the explorer and inspector share the
lower row. At 640px and below, the body shows one of Robots, View, or Inspect at
a time via a three-part surface switcher. The rail remains outside that switch,
caps at 42dvh, and scrolls internally. Record grows to a 44px touch target and
the selected-episode facts and actions stack to full width. Touch has no hover,
so revealed actions stay visible there: Delete on an episode card, which grows
to 120×44px to hold a 44px target without taking the room its labels need, and
the camera row's unassign mark, which grows to 28px.

### Named Rules

**The Literal-Loop Rule.** The primary session sequence is Record → select an
episode → Replay → Export. Do not fork it into judgement, diagnostics, or
analysis lanes.

**The No-Flung-Pair Rule.** A wide strip does not hold two clusters pinned to
opposite edges with nothing between them. Either something elastic fills the
middle — the episode strip does exactly this in the rail's first row — or the
row packs from the left and simply ends. A lone control a thousand pixels from
the thing it acts on reads as breakage, not as breathing room.

**The Equal-Episode Rule.** Every episode button has the same footprint.
Duration, completion, and capture order are text or state, never proportional
bar geometry.

**The No-Unfolding-Prose Rule.** Explanatory text does not appear and disappear
with a live value. A sentence that unfolds when a measurement crosses a
threshold, or when a mode changes, pushes everything below it down a row and
back — under a 20Hz telemetry stream that is the whole panel breathing. State
that changes gets a tint, a chip, or a disabled control; prose is reserved for
what stays true for as long as the operator is looking at it. The single
survivor on the joint panel is the uncalibrated-arm line, because it changes
once, when the arm is calibrated.

**The Nothing-Load-Bearing-Hidden Rule.** A breakpoint may reflow, reorder,
cap, or scroll. It may not hide a control, safety state, blocker, or active
recording. Collapsing the rail during recording leaves its transport visible.
A horizontal scroller counts as hiding: it shears a control off an edge with
nothing on screen saying the control is still there. At the narrowest widths the
status cluster may drop the loop rate, and nothing else — the environment,
outstanding issues, a fault, and any arm holding torque stay on the page,
because the control that energizes an arm is still there and still enabled.

**The Consequence-On-The-Page Rule.** Anything that describes what a control
will do to the hardware — energize, release, stop, discard — is rendered text,
not a `title`. An operator with a hand on the arm has no hand for a hover, and
on a touch bench the tooltip does not exist at all. Releasing torque is confirmed
in place with its instruction visible, the same way deleting a take is. The
boundary is consequence and blockage, not curiosity: what a measurement *means*,
and a note about a step already completed, may live in a `title`.

## Elevation & Depth

The studio is flat by default. Depth comes from paper, pale-bench, and recess
tonal steps plus one-pixel hairlines. Selection uses a wash and a coincident
inset ring so state does not shift layout.

### Shadow Vocabulary

- **Resting lift** (`0 1px 2px rgb(0 0 0 / 0.05)`): Small objects placed above
  a surface, including the view cube, reconnect card, and slider thumb.
- **Segment lift** (`0 1px 3px {colors.lift}`): The active mobile surface
  switcher tile.
- **Floating notice** (`0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)`): Anchored issue popovers and bottom notices only.
- **Inset ring** (`inset 0 0 0 1px currentColor`): Selected episode cards and
  disabled filled actions; it is state, not elevation.

### Named Rules

**The Flat-by-Default Rule.** Resting panels, cards, controls, and the dataset
rail do not cast shadows. Shadow means an object genuinely floats above the
shell.

**The Inset-State Rule.** Selection and disabled treatments happen inside the
existing box with a wash, outline, or inset ring; state changes do not add a
border that moves adjacent content.

## Shapes

Most controls, fields, cards, episode buttons, chips, viewport chrome, and
popover corners use a small 4px radius. Floating notices use 6px. Status dots,
slider thumbs, and circular issue marks are fully round. The mobile segmented
switcher is slightly softer because it is a container around three controls,
but its active tile remains compact rather than pill-shaped.

Borders are one-pixel role-token hairlines. Icons are inline 16×16 SVGs with a
1.35px stroke, round caps and joins, and no fill except where the transport mark
needs a solid record/stop symbol. They draw at 14px in chrome and 12px inside a
dense row's revealed action. Form follows hierarchy: shallow horizontal groups,
aligned rows, and compact rectangular targets rather than bubbles, oversized
cards, or ornamental silhouettes.

## Components

### Buttons

- **Shape:** Compact rectangle, 4px radius, 24px minimum height, 6px horizontal
  padding, and a transparent resting border so variants share a box. Record is
  the single 28px exception.
- **Primary:** Role-blue fill with white text. Use once per local purpose:
  starting teleoperation, Connect, Record, or Replay.
- **Hover / Focus:** Quiet controls gain the hover wash; primary controls
  brighten without changing hue. All retain the global 2px focus ring at 1px
  offset — one pixel, not two, because at a 24px control height a 2px gap reaches
  into the neighbouring control and reads as if both were focused.
- **Danger:** Red text on transparent, with a red soft wash on hover. Delete
  requires inline confirmation; it is never a filled red call to action.
- **Link:** The quiet text action — Rescan, Identify, Export, Delete, Dismiss.
  11px soft graphite at the 24px target height, with no border or fill,
  darkening to primary ink on hover. Destructive links take danger ink on hover
  instead. This is where a rare action belongs, so that being a button never
  makes the rarest action in a row the loudest thing in it.
- **Disabled:** A reason appears beside Record, Replay, or Export. The Record
  fill drops to recess with faint ink and an inset hairline rather than relying
  on global opacity.

### Chips

- **Style:** 11px text, 4px radius, 0/4px padding, hover-wash background, and
  soft graphite ink. Chips stay at 11px rather than dropping to micro: at narrow
  widths they are the only place torque state appears.
- **State:** Blue wash for active, ochre wash for simulation or an unverifiable
  environment, and red wash for fault or torque danger. A 6px dot carries simple
  connection/completion state.
- **Presence is meaning.** Per The Exceptional-Status Rule, a chip renders only
  while its condition is exceptional. In the joint inspector, a teleoperation
  chip names what the arm is *doing* — "leading", "following" — rather than
  repeating an identity the arm's own name beside it already gives.

### Readout Cell

- **Style:** A recess-filled 4px cell holding a 10px faint label over an 11px
  tabular mono value, four across the top of the joint inspector.
- **Unit:** A separate faint element rather than part of the value string, so
  four cells line their digits up instead of each starting wherever its own
  label ended. The digits hold 3.5ch so the unit stays at a fixed x as the value
  changes.
- **Caution:** An out-of-range reading swaps recess for the ochre wash and
  graphite for ochre ink. The tint is the entire treatment; nothing is added to
  the layout.

### Cards / Containers

- **Panels:** White surfaces divided by hairlines; first headers stick while
  their content scrolls.
- **Setup group:** A recess block under an expanded robot entry, divided into
  hairline-separated sections. Each section opens with the same 24px head — its
  name on the left, its quiet link actions packed right — and only carries an
  instruction line where there is still work to do. A satisfied step is its
  heading and its button, and nothing else.
- **Episode card:** Fixed 72×36px, white on the rail, with three facts on two
  lines: short ID and duration on one baseline, completion state beneath — all
  three fully legible, because the card reserves no permanent room for its
  delete affordance. Selected cards use the blue wash plus inset blue ring. Live
  recording uses the danger wash and the 1.6s `take-pulse` on border and
  background; cancelled legacy episodes use recess and faint ink. Delete appears
  on hover or focus and confirms in place, in an overlay sized to two full-height
  buttons rather than to the card.
- **Camera property card:** White, hairline-bordered, 4px corners, with a 16:9
  dark camera mat, one per row at the panel's full width. Selection aligns
  border and inset outline to role blue.
- **Notice / issue popover:** White, hairline-bordered, modest shadow, and dense
  text. The desktop issue popover is 320px wide and anchored under the header
  summary; on mobile it becomes fixed between 8px side gutters. A notice carries
  its own expiry and comes off the page on its own.

### Inputs / Fields

- **Style:** 24px minimum height, white fill, graphite ink, 4px radius, 1px
  hairline, and 0/6px padding.
- **Focus:** Border shifts to role blue while the global focus ring remains.
- **Inline variant:** Pickers inside a bordered toolbar group shed their own
  border and background.
- **Paired action:** A picker whose purpose is completed by one button shares its
  line with that button — the port select and Connect — rather than stacking a
  status row above and an action row below.
- **Slider:** A 3px hairline track and 12px soft-graphite thumb with a 2px paper
  ring, in a 20px row.

### Navigation

There is no site navigation. Pane visibility is application chrome: three 24px
icon toggles in the header, with pressed state shown as role blue on the selected
wash. Below 640px, a Robots / View / Inspect segmented switcher replaces the two
side-pane toggles; the dataset rail toggle remains independently available.

### The Dataset Rail

The rail is the signature shipped component because it makes the product loop
literal without turning recordings into an analysis surface.

- **Record transport:** A 28px blue button with the record mark, “Record,” and
  `R` shortcut; on mobile it grows to 44px. During capture it becomes red and
  reads “Stop & save.” Cancel remains a secondary mid-recording escape hatch.
- **Timer:** 16px tabular mono with a 10px supporting label on the same
  baseline, carrying the shortcut or the live frame count.
- **Session summary:** Exportable episode count and total duration followed by
  the outlined Export dataset action. Naming is an inline field only after the
  operator invokes Export.
- **Episode strip:** Equal-size, horizontally scrolling buttons in oldest-first
  capture order, limited to the newest 120 items when necessary, with the
  remainder counted on a dashed card at the head of the strip. Empty state
  tells the operator to start teleoperation and press Record or `R`.
- **Selected row:** Identity and completion dot, four compact facts, then a
  target-specific action such as “Replay on follower.” Replay selects a
  compatible calibrated arm and prints any blocker beside the disabled action.
  While this episode is the one playing, the action becomes fixed-width frame
  progress and Stop: the escape from motion stays where the motion was started,
  and the counter does not walk it sideways as it counts.
- **Workflow boundary:** Incomplete and cancelled legacy files remain visible
  and excluded from export, but there is no primary keep/reject judgement lane,
  tracking trace, error graticule, or quality verdict.

### The WebGL Bench

The stage uses a pale neutral clear color with broad, low-contrast lighting so
white printed shells remain distinct. The grid is on by default and remains an
explicit control: the bench is a physical workspace, and one sparse 20cm rhythm
plus subdued axes is how reach and clearance get read off the screen. Arm accents
connect the legend to geometry. Chrome over the bench is two corner clusters and
two edge notes, not six floating boxes: Grid and Home share one group at the top
right, the axis key is the last row of the view cube rather than a separate card
above it, the arm legend sits at the top left, and the interaction hint runs
along the bottom left. All of it uses the same light surface and graphite
vocabulary as the rest of the app.
The cube's faces are spelled out — Top, Front, Right — never abbreviated to
single letters that would read as keyboard shortcuts beside the ones that are.

### Named Rules

**The Focusable-Reveal Rule.** A control revealed on hover fades in with
`opacity`; it is never hidden with `visibility` or `display`. A hidden element
cannot take focus, so hiding one outright puts it beyond a keyboard entirely.
The episode card survives this because focusing its visible episode button trips
`:focus-within` on the card, but a row whose only focusable child *is* the
revealed control has no such escape — so it reveals on its own `:focus` as well
as the row's hover, and stays permanently visible where there is no hover at all.

**The Expiring-Notice Rule.** A transient notice carries its own deadline and is
taken off the page by the studio clock — 4 to 14 seconds, scaled to reading
length, with a longer floor for a failure that asks for a decision. Dismiss
stays, for the operator who is faster than the timer. The deadline lives in the
notice value rather than in the component that draws it, so a notice cannot be
raised without one.

## Do's and Don'ts

### Do:

- **Do** keep the shell, WebGL bench, and dataset rail in one light neutral
  world; reserve the dark mat for camera imagery.
- **Do** keep Record and Replay as the key local filled actions and Export as
  the session's explicit exit action.
- **Do** use equal-size episode buttons and print duration explicitly.
- **Do** keep the base at 12px with 24px controls and a 2/4/6/8px rhythm.
- **Do** keep type on the four documented steps; reach for weight and the
  graphite ramp before reaching for a size.
- **Do** set changing numbers, IDs, rates, durations, and counts in tabular mono.
- **Do** give every telemetry-fed value a fixed box, so a dropped digit moves
  nothing beside it.
- **Do** render a status chip only while its condition is exceptional; the
  resting state is plain faint text or nothing.
- **Do** let transient notices expire on their own while keeping Dismiss.
- **Do** put disabled-action reasons on the page beside Record, Replay, and
  Export.
- **Do** keep the scale grid sparse and user-controlled, on by default.
- **Do** place warnings in the compact header popover and keep transient notices
  floating above the shell.
- **Do** preserve device hierarchy: identity and readiness first, setup second,
  file maintenance last.
- **Do** reflow, cap, and scroll at narrow widths while keeping active recording
  and safety state visible.
- **Do** keep `tailwind_input` and `tailwind_output` declared in
  `Dioxus.toml`; DX owns stylesheet compilation for serve, build, and bundle.

### Don't:

- **Don't** reintroduce a fixed-dark viewport or dark dataset rail.
- **Don't** enlarge the elapsed timer beyond the shipped 16px role.
- **Don't** add a heading row whose text restates what the region below it
  obviously is, or a status line whose words the row above already carries.
- **Don't** pin one control to a strip's far edge when the middle is empty; fill
  the middle or pack the row left.
- **Don't** let a value and its unit wrap: units are their own faint element so
  neighbouring readouts keep their digits aligned.
- **Don't** unfold a sentence when a live value crosses a threshold or a mode
  changes; the layout below it must not move under telemetry.
- **Don't** print an instruction that points at a visible, labelled control a
  few pixels away.
- **Don't** hide a hover-revealed control with `visibility` or `display` when it
  is the only focusable thing in its row.
- **Don't** make judgement, keep/reject verdicts, tracking traces, thresholds,
  graticules, or diagnostic charts primary dataset UI.
- **Don't** encode episode duration or quality in card width or height.
- **Don't** show persistent warning banners between the header and workspace.
- **Don't** add dense minor grid lines to the bench.
- **Don't** write one-off colour literals or hue-named utilities in components;
  use the role tokens.
- **Don't** rely on a `title` attribute as the only explanation for a hardware
  consequence, a safety state, or a blocker.
- **Don't** hide a control, state, blocker, or active recording at a breakpoint.
- **Don't** add a second button, chip, icon, or type vocabulary for a new
  surface; reuse the compact system.
- **Don't** add uppercase tracked kickers, decorative display type, pill-shaped
  containers, or ornamental animation. The live episode pulse is the only
  authored looping motion and respects reduced-motion preference.
