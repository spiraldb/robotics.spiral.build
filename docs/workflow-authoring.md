# Workflow authoring

A workflow owns a SQL definition and one Assistant conversation. SQL is the only
executable representation. Spiral owns decoding, inference, optimization, and
query execution; Studio owns authoring, plan display, run identity, and results.

## Documents and ownership

The browser owns the current draft, saved baseline, and Assistant loop. Direct
SQL edits and `set_workflow` change the same draft. Save atomically persists SQL,
its native plan display snapshot, and conversation with optimistic revision
checking. Run submits an immutable copy of the visible SQL, including an unsaved
draft, without saving it. Editing
or saving during execution cannot change the submitted query or its results.
Save or Discard before switching documents. Unsent chat text stays local to its
workflow. Page reload loses unsaved edits; submitted backend runs continue.

The existing Spiral `workflows` relation keeps its column names (`graph_version`
and `graph_json`), but encoding version 4 contains a `workflow` with `id`,
`name`, `revision`, and `sql`, plus an optional `planned_sql` containing the exact
SQL and native plan report/error. Earlier SQL documents without this additive
field are planned on opening and include it on their next Save. Conversation
encoding is independent. There is no saved runtime snapshot. Older graph
documents are preserved and refused with a
reauthoring message. They are never silently converted or executed by a fallback.
An incompatible saved document does not prevent creating a new SQL workflow.
This cutover stores SQL literals; a separate bound-parameter editor is not implemented.

## Assistant investigation and plan inspection

The four dockable panes are Graph, Results, Assistant, and SQL. Assistant and
SQL share a tall 24% column at the left. The graph owns the upper tile of the
remaining width (46% of the height) and reads left to right; selecting a node
or an edge floats its details in a sheet beside the card instead of a separate
pane, so selection never resizes the canvas, including keyboard selection.
Results run along the bottom 54%. SQL changes clear the inspection. The graph
has no navigation toolbar; native panning, zooming, keyboard interaction,
zoom/fit controls, and a minimap when the graph does not fit at readable zoom
remain available. The workbench keeps all splits resizable
and all panels dockable.
One document bar owns the workflow name (which opens the switcher, creation, and
rename popover), the saved/draft state, Save, Discard, and Run. Results owns the
selected run, its phase, row count, elapsed time, and Executed SQL, and shows
skeleton rows while a run is admitted. Result cells are painted by inferred kind
(images, categorical chips, measurement bars, UTC timestamps, mono identifiers);
the inference is display-only and every tooltip and the row inspector keep the
stored value exact.
A request to find, evaluate, or filter data authorizes the Assistant to run the
required read-only queries. The Assistant receives the installed `spql.ai` function catalog each turn,
including signatures, metadata, and descriptions. Metadata tools remain
`inspect_dataset`, `list_relations`, `describe_relations`, and
`list_model_capabilities`.

The investigation loop is inspect → `set_workflow` → `run_workflow` → `inspect_run`
→ evaluate → revise → `show_results`. `set_workflow` plans candidates before changing
the draft; errors leave it unchanged. `run_workflow` submits the visible SQL once
through the same native runner as manual Run and returns an immutable run ID.
`inspect_run` waits for that ID and returns paged preview rows, exact SQL, errors,
and truncation. Without an ID it lists this workflow's admitted runs, including
runs whose submission response was lost. It never reruns a query.

`inspect_images` selects 1–4 actual image preview cells by run, column, and zero-based
row index. Native image/tensor preview conversion is shared with the Results viewer.
Images are attached to the next model request only; saved history contains evidence
references and the model's text observations, not encoded payloads. Missing previews,
failed runs, and references to another workflow fail explicitly. The agent uses
small SQL-selected evaluations, targeted counts, and representative frames before
processing the requested cohort. Preview limits bound display, not computation.

`show_results` selects a successful run and restores its producing SQL, even after
later evidence queries. Results has a run selector and an Executed SQL disclosure;
selecting an older result does not silently edit the draft. The plan follows the
current draft. Stale agent proposals cannot overwrite intervening user edits.
The final response should state the answer, evidence, assumptions, unknowns, and
preview limitations. Planning or successful execution alone is not task completion.
Requests only to explain or draft SQL need not execute it. Missing capabilities or
resources remain explicit blockers.

Only Save persists SQL, its plan snapshot, and conversation. The browser reuses the
plan produced by `set_workflow` for graph display and Save. Reopening a saved
workflow reuses its plan without issuing EXPLAIN. SQL changes invalidate reuse;
renaming or advancing the saved revision does not. Save obtains a plan if none
matches the submitted SQL, and a transport failure leaves the document unsaved.
The saved plan describes the schema/resources at planning time; Run still submits
SQL to Spiral for current planning and execution. Plan snapshots are display data,
not executable plans or durable run results. New workflow starts blank; creating one
from a dataset starts with a native `spql.mcap.episodes` query. The Assistant discovers
the actual recorded camera columns and uses native video and AI functions.

`EXPLAIN (FORMAT JSON)` supplies version 2 native diagnostics. Logical operators
now carry additive `properties.expressions` metadata with its own schema version 1:
named roots, expression occurrence IDs, ordered argument edges, structured function
set/overload identity, display text, types, and a completeness flag. Expression IDs
are local to one relational operator, and parents precede their arguments. This
requires a local ADBC driver rebuilt with the expression exporter.

Studio defaults to Workflow, a presentation derived from the logical plan's
relational and expression dependencies. Native identities identify SPQL operations
such as image decoding and luminance measurement; nested calls retain their
dependencies and independent calls stay separate. Routine arithmetic, predicates,
and standard scalar functions stay within SQL stages. Linear routine SQL chains
collapse without collapsing joins or shared branches. Conditional calls display
their guard. For the inspected dim-image query, six native operators become four
stages: Read data → Decode images → Measure luminance → Classify & order; the last
stage retains three SQL operations. Stage summaries describe the query's own rules,
not inferred classifications.

The Graph pane shows only the simplified Workflow projection. Operator details and
Full plan JSON preserve the native diagnostics, including operator IDs and ordered
inputs. Missing, unsupported, invalid, or incomplete expression metadata blocks
the projection explicitly. Subqueries and opaque plugin expressions can mark
coverage incomplete. The graph is a read-only diagnostic; SQL remains the
executable definition.

Dioxus Flow owns layout, panning, zooming, fitting, and selection. Initial zoom is
bounded to 0.75–1.0 and refits while the dock layout settles. Large graphs begin at
the source and remain pannable, with a native minimap above six stages. Once the
user navigates, resizing the pane preserves graph-space center and zoom.
Selecting a node floats the stage's sheet beside it: summary, facts, folded
steps, the columns it hands on, and its native operators behind one disclosure.
Each operator retains its stacked schema, properties, optional detail text, and
execution identity. New SQL resets the plan and selection. IDs belong to one graph, not to a
workflow revision, so an old ID cannot select an unrelated stage.

Assistant activity is grouped by user request. The latest answer stays visible;
earlier findings, tool outcomes, and result receipts remain in one activity
disclosure. CommonMark headings, lists, code, and tables use pulldown-cmark followed
by ammonia sanitization; image tags are removed and tables are disclosed.
The underlying conversation is unchanged.

Data and Workflow use the same query result table. Rows place native image columns first, followed by decision,
status, and label columns. Identifiers and paths remain available later in the
same table. Column types are optional; row-number and image actions open exact
values and larger images in a local inspector, moving focus to its heading.
Close or Escape returns focus to the invoking action. Floating-point display precision is reduced
only in the table; exact integers, stored values, and row ordering are preserved.
Changing the selected run, query tab, or result snapshot resets the row inspector.
There is one rendering path for loading, errors, empty results, and populated tables;
neither activity defines a separate result presentation mode.

Studio preserves the native report's additional diagnostics and does not parse
text to recover topology. Expression identities and dependencies are structured;
expression display text, type descriptions, and opaque properties remain native
display strings. These diagnostic graphs are not an
executable serialization. When physical lowering fails, the logical plans remain
inspectable alongside the error. Unsupported report versions fail explicitly.
Planning does not execute inference. Graphviz and its server process are removed.

## Native execution

Run submits the exact SQL once through the existing ADBC query connection.
Studio retains one active workflow run, mutual exclusion with recording, and
session-owned immutable observations. It shows whole-query phase, elapsed time,
and a bounded final preview (100 rows plus transport byte limits). It drains the
remaining result stream, so a preview limit does not stop inference or hide a
late failure. Use SQL selection or LIMIT to bound requested computation.

There is no workflow Cancel action in this cutover, as agreed while upstream
ADBC cancellation is unavailable. Backend/project shutdown waits for an admitted
workflow run to finish. The Assistant's Stop action aborts its current investigation (model request or result wait), but an admitted query continues and remains observable. The UI states this while a query is active.
Studio does not invent per-operator progress or persist previews as training inputs.
Native reuse and durable completed outputs remain milestones 3.2 and 3.3.

The Studio graph scheduler, Model operator, SQL-node fusion, `{{node:id}}`
bindings, and intermediate Arrow routing are removed. Existing legacy cache data
and storage/export contracts are preserved but do not execute workflows.

The current upstream native scoring function is `spql.ai.score`. Its default
CLIP image/text scores are unnormalized logits, not the retired Studio SigLIP
scores, calibrated probabilities, or measured brightness. Use catalog signatures
and native defaults; explain provisional thresholds instead of reusing SigLIP
thresholds. Null inputs yield null scores. Actual non-null model execution and
full bright/dark episode evaluation require the upstream model resources.

## Verification

Native tests cover exact unsaved SQL execution, immutable runs, one-run admission,
plan errors without creating runs, native capability discovery, structured plan decoding,
and null-input AI execution. Browser tests cover the real Assistant fetch/tool loop through native execution errors,
SQL revision, result inspection, transient image attachments, final selection of an
earlier run, stale-edit rejection, and stopping while an admitted query continues. Save/revision and
conversation tests continue to cover document ownership. Full episode inference
is separate from these checks.

The local ADBC driver was rebuilt with the additive native expression metadata.
Graph tests cover native projection, nested and independent calls, repeated/shared
inputs, conditional guards, literal function-name text, missing/invalid metadata,
and incomplete coverage across operator kinds. The latest seven graph tests pass;
132 frontend tests passed before the final completeness guard. Native metadata
and inference regression checks, frontend/native Clippy, both formatting checks,
native documentation lint, and the final release web build pass.

Cold-load visual review passed at desktop widths of 1280 and 1511 pixels. Browser
checks covered graph modes, schema details, row-image inspection with Escape focus
restoration, and a 30-row read-only dim-image query displayed at 4.3 seconds.
Manual graph-space center and zoom were preserved across the 1280→1511 resize and
Details open/close; no console errors were observed. This verifies those desktop
states, not mobile layouts or every workflow. Full non-null episode inference
remains separate from this UI change.

September 11 agent-loop verification: 256 native tests and 124 browser tests passed.
Live agent evaluation recovered from a native decimal-cast error and decoded nine
frames from three episodes. Non-null `spql.ai.score` then failed because Spiral's
Hugging Face credential service was unavailable. This is an upstream blocker, not a
completed dim-lighting filter or a reason to substitute Studio-side inference.

The live continuation decoded frame-0 evidence for all 52 episodes, inspected selected
images through the configured model, and used `show_results` to present the complete
52-row report with null scores and unknown lighting decisions. No dim/bright filter
was validated. The initial trial also exposed the old 16-step authoring limit; the
investigation loop now allows 32 steps (96 tool calls), with an explicit limit error.
