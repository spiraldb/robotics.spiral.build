# Workflow authoring

A workflow owns a SQL definition and one Assistant conversation. SQL is the only
executable representation. Spiral owns decoding, inference, optimization, and
query execution; Studio owns authoring, plan display, run identity, and results.

## Documents and ownership

The browser owns the current draft, saved baseline, and Assistant loop. Direct
SQL edits and `set_workflow` change the same draft. Save atomically persists SQL
and conversation with optimistic revision checking. Run submits an immutable
copy of the visible SQL, including an unsaved draft, without saving it. Editing
or saving during execution cannot change the submitted query or its results.
Save or Discard before switching documents. Unsent chat text stays local to its
workflow. Page reload loses unsaved edits; submitted backend runs continue.

The existing Spiral `workflows` relation keeps its column names (`graph_version`
and `graph_json`), but encoding version 4 contains only a `workflow` with `id`,
`name`, `revision`, and `sql`. Conversation encoding is independent. There is no
saved runtime snapshot. Older graph documents are preserved and refused with a
reauthoring message. They are never silently converted or executed by a fallback.
An incompatible saved document does not prevent creating a new SQL workflow.
This cutover stores SQL literals; a separate bound-parameter editor is not implemented.

## Assistant investigation and plan inspection

The four dockable panes are Assistant, Query plan, SQL, and Results. A request to
find, evaluate, or filter data authorizes the Assistant to run the required read-only
queries. The Assistant receives the installed `spql.ai` function catalog each turn,
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

Only Save persists SQL and conversation. New workflow starts blank; creating one
from a dataset starts with a native `spql.mcap.episodes` query. The Assistant discovers
the actual recorded camera columns and uses native video and AI functions.

`EXPLAIN (FORMAT JSON)` supplies version 2 native diagnostics. Studio displays
its logical graph by default, with optimized logical and physical views available
in the plan selector. Dioxus Flow owns layout, panning, zooming, fitting, and node
selection. Native IDs and ordered input edges are preserved, including repeated
inputs and shared subplans. Node IDs belong to one graph, not to a workflow revision.
The graph is read-only; SQL remains the executable definition.

Selecting an operator shows its output schema, properties, optional detail text,
and available execution identity. Full native report JSON is also inspectable.
The inspector stacks below the graph in narrow panels. New SQL resets the plan
and selection, so an old node ID cannot select an unrelated operator.

Studio preserves the native report's additional diagnostics and does not parse
text to recover topology. Expressions, type descriptions, and some opaque
properties remain native display strings; these diagnostic graphs are not an
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

The structured-plan cutover uses the current local driver at `41e373f4b`, which
includes JSON diagnostic graph support. Verification checks native plan decoding,
shared and repeated inputs, read-only Flow projection, and incomplete physical
plans. Full non-null episode inference remains separate from this UI change.

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
