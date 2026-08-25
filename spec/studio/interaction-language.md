# Studio interaction language

## Outcome

Studio uses one interaction language across the shell, docked panels, Live,
Data, Workflow, Training, Rollout, and Settings. A user can predict an action's
meaning, location, state, confirmation, and focus behavior from its verb.

## Action grammar

| Verb | Meaning |
| --- | --- |
| Create | Make a durable entity. |
| Assign | Create a relationship between existing entities. |
| Connect / Disconnect | Change a live device connection. |
| Remove / Unassign | Detach a reversible relationship. |
| Close | Dismiss a view while preserving its data. |
| Delete | Permanently remove a durable entity. |
| Discard | Abandon uncommitted work. |
| Apply | Commit configuration or calibration. |
| Replace | Overwrite an existing scope. |
| Hold / Teleoperate / Release | Change physical motor control; Release turns torque off. |
| Sync | Reconcile clocks or state. |
| Fit | Frame the current spatial contents inside their viewport. |
| Retry / Refresh | Repeat failed work or reload current state. |

Start, Stop, Cancel, Import, Export, Navigate, Select, and Toggle keep their
literal meanings. A visible label may be more specific, such as **Run query**,
while its typed purpose remains Start. Workflow Assistant **Send** also uses
Start; its **Stop** ends the active Assistant turn and any still-active
Workflow run owned by that turn.

Panel reordering is a workspace gesture with pointer and keyboard paths, not a
product `Action` purpose.

## Placement

| Scope | Placement |
| --- | --- |
| View | Panel, dialog, or view header. |
| Collection | Collection toolbar or list footer. |
| Row or item | Trailing edge of that row or item. |
| Editor commit | Editor or dialog footer. |
| Physical safety | The control region that owns the physical mode. |

An uncommon action may use overflow when the surface has one. Moving an action
into overflow must not change its verb or confirmation level.

## Confirmation levels

- **Immediate:** Recoverable actions run without confirmation.
- **Local destructive:** A permanent item-level action uses the shared inline
  confirmation at the item's location.
- **Blocking:** A scope-wide replacement or physical-safety consequence uses
  the shared modal alert dialog.

Every destructive confirmation names the action, target, and consequence. The
safe action receives initial focus. Escape cancels when no request is pending.
Cancel returns focus to the initiating action. Studio must not use native
browser confirmation dialogs.

## Requirements

- **STUDIO-027** Every product action must use a typed primitive from
  `frontend interaction components` and declare its semantic purpose.
- **STUDIO-028** A visual variant must not change an action's verb.
- **STUDIO-029** A feature may define action placement and layout but must not
  redefine shared hover, focus, selected, disabled, pending, or destructive
  states.
- **STUDIO-030** Close, Remove, Unassign, Delete, Discard, and Replace must be
  used according to the action grammar.
- **STUDIO-031** Local permanent deletion must use the shared inline
  confirmation.
- **STUDIO-032** Scope-wide replacement and motor Release must use the shared
  blocking confirmation.
- **STUDIO-033** A confirmation must keep its target and consequence visible
  while pending, block duplicate submission, and remain available after a
  failure.
- **STUDIO-034** Selection, toggle, expanded, and mode state must be exposed
  semantically and must not depend on color alone.
- **STUDIO-035** A mutually exclusive physical mode must use radio-group
  semantics and support arrow-key selection.
- **STUDIO-036** Hover may reveal a quiet trailing action only when keyboard
  focus also reveals it.
- **STUDIO-037** Controls must preserve the compact desktop target scale.
- **STUDIO-038** Panel reordering and docking must work with pointer drag and
  the documented keyboard alternatives.
- **STUDIO-039** Studio is a strict desktop-only interface with a 1024
  CSS-pixel minimum viewport. Product code must not add mobile or touch-specific
  layouts, breakpoints, gestures, hit-target overrides, or alternate UI.
- **STUDIO-040** Readiness language must never claim Ready when a blocker or
  missing dependency prevents the action.
- **STUDIO-041** Pending actions must expose `aria-busy`, remain in place, and
  prevent repeat submission.
- **STUDIO-042** Reduced-motion mode must remove decorative movement without
  collapsing meaningful state changes to near-zero animation durations.
- **STUDIO-043** Automated checks must reject raw product buttons, raw file
  pickers, native browser confirms, and retired action classes.
- **STUDIO-044** Product components and local CSS classes must be named for
  domain intent or layout responsibility, not their native presentation.
- **STUDIO-045** Cross-surface icons, dialog surfaces, focus movement,
  single-flight pending behavior, and value formatting must have one shared
  owner rather than feature-local copies.
- **STUDIO-046** Unused components, properties, variants, selectors, tokens,
  and dependency features must be removed when their final consumer is removed.
- **STUDIO-047** An external-service transport failure must name the unavailable
  service and configured endpoint, give the next recovery action, and keep the
  underlying diagnostic in backend logs instead of presenting driver internals
  as the primary interface message.
- **STUDIO-048** A transient status-bar message must have a shrink-safe lane and
  must not render beneath operational state. Rapidly changing numeric telemetry
  must reserve a stable tabular width so updates do not shift adjacent content.
- **STUDIO-049** When a blocking operation fails, its confirmation layer must
  dismiss, return focus to the initiating control, and present the failure in
  the owning panel so the user can retry or leave without a trapped overlay.
- **STUDIO-050** Each activity-rail action must center its icon-and-label stack
  vertically and horizontally within the full navigation target.
- **STUDIO-057** A Settings save or connection-test failure must remain visible
  in the owning Settings panel beside its retry and exit actions; a transient
  status-bar message may supplement it but must not be its only presentation.
- **STUDIO-063** Workflow node selection must use the shared selected and focus
  states, support pointer activation plus Enter or Space from keyboard focus,
  and open the Node inspector only after explicit selection. Initial rendering
  must leave Assistant active.
- **STUDIO-066** Workflow Assistant Send, Stop, and Retry must use the shared
  typed actions and single-flight pending states. Plain Enter submits the
  composer; Shift+Enter inserts a new line, and the shortcut remains visible
  beside the action. The compact **Conversation** selector and typed **New** action must
  preserve older conversations, keep one shell-hoisted draft per project scope and conversation
  across activity and Settings view unmounts, clear only an accepted draft, and expose
  the exact disabled reason at the 20-conversation bound, during active Agent
  work, or when Spiral history is unavailable. Provider-unconfigured alone
  must not disable conversation management. Switching conversations must not
  change the Workflow graph, run, Results, or panel focus. The
  current streaming message and live turn phase may announce without replaying
  restored history. Assistant prose must render safe typed
  paragraphs/lists/emphasis/inline-code/code-block nodes, never raw HTML.
  Completed tool summaries collapse behind a native disclosure while running
  or failed work remains open. Near-bottom threads follow streaming work; a
  user who scrolls away gets an explicit **Jump to latest** recovery. Tool summaries must use human bounded receipts; matching saved-graph, run,
  or result receipts may expose explicit Navigate actions
  without stealing focus, while stale receipts remain inert. An unavailable provider must keep Send disabled
  with bounded temporary-unavailable copy on the page, never provider, model,
  environment-variable, or backend-configuration instructions.
- **STUDIO-068** Workflow Results rows with one or more exact
  `spql.image.Image` fields must keep the complete table and render every image
  directly in its corresponding schema cell. Image cells use bounded previews
  and contextual episode/stream/frame alt text; other cells retain their normal
  values with type-aware headers, aligned numbers, quiet nulls, and one shared
  result metadata strip.

## Implementation ownership

- Typed actions, toggles, modes, confirmations, focus behavior, and shared
  states: [`frontend interaction components`](../../crates/spiral-robotics-frontend/src/components/interaction/)
- Product placement and domain content:
  [`frontend`](../../crates/spiral-robotics-frontend/src/)
- Shared product formatting and single-flight request behavior:
  [`format.rs`](../../crates/spiral-robotics-frontend/src/format.rs) and
  [`tasks.rs`](../../crates/spiral-robotics-frontend/src/tasks.rs)
- Docking, tab reordering, and panel keyboard behavior:
  [`dioxus-workbench`](https://github.com/XiangpengHao/dioxus-workbench) (external crate)
- Migration guard:
  [`interaction_language.rs`](../../crates/spiral-robotics-frontend/tests/interaction_language.rs)
