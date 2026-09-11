# Workflow authoring

A workflow is the unit the engineer creates, names, selects, and returns to.
It owns one executable graph and one Assistant conversation. Graph nodes and edges
are arrays inside JSON, not separate database tables.

## Storage and ownership

The Spiral `workflows` relation stores one row per workflow: `workflow_id`, `name`,
`graph_version`, `graph_json`, `conversation_version`, `conversation_json`, and
`updated_ms`. Graph nodes and edges remain inside the graph JSON. Conversation JSON
contains the visible messages and provider history for one Assistant conversation.

The frontend reads IDs and names ordered by the last explicit Save and opens the
first document. An empty list creates a browser-owned draft. New, selection,
editing, and Discard do not mutate a backend editing session; no such session exists.
Loading a document reads its graph and conversation together in one SQL query,
so both come from the same saved row without a second database round trip.

**Save is the only authoring persistence action.** It atomically stores the graph
and Assistant conversation in one row. The graph revision is the saved document
version, checked again within the storage transaction before replacement. A stale
save fails rather than overwriting another save. Invalid or incomplete drafts may
be saved; execution still requires a valid graph.

The browser owns the current graph, its last saved baseline, conversation, and
agent loop. Manual edits and `set_workflow` update the same draft. The loop sends
one completion request at a time to `/models/complete`; that backend endpoint owns
credentials and provider streaming, but no conversation or tools. Read-only tool
calls use ordinary dataset/query capabilities. Graph changes first call
`/workflows/plan`, then compare the local draft against the input used to author
the change. Intervening user edits are preserved and reported as a conflict.

Run submits the complete visible graph once to `/workflow-runs`. The backend owns
an immutable copy and returns a run ID. Run observations are separate from saved
documents and remain available for the backend project session. They include the
submitted graph, including for never-saved workflows. Editing, saving, or selecting
another graph cannot alter an existing run. Only one execution is admitted at a
time; recording and execution remain mutually exclusive. The authoring assistant
does not reserve hardware or execution admission.

Save and Discard affect the graph and conversation, not running jobs or results.
The saved graph envelope remains version 3 (with an idle snapshot for compatibility).
Compatible version-2 SQL graphs still open; unsupported documents are refused
without rewriting them. Encoding version and saved document revision are separate.

Save or Discard before switching documents. The frontend retains workspaces during
navigation. Closing/reloading the page loses unsaved drafts and conversation
changes and stops its agent loop. Submitted backend runs continue until completion,
cancellation, or backend/project shutdown. Closing the model response body cancels
the request; it never leaves a server-owned agent loop behind.

Unsent chat text is local to its workflow and is not part of a saved conversation.

## Authoring and execution

There is one intent graph and one Run action. The Assistant can inspect datasets,
relations, model capabilities, and bounded read-only results, then update the graph in memory.
It cannot start or wait for a workflow run. Run and Cancel own computation.

The graph has four node kinds:

- **Table:** shorthand for a SQL source over a qualified relation.
- **Query:** a SQL source with no incoming edges, or a transform over connected
  `{{node:id}}` inputs. SQL owns filtering, sampling, projection, aggregation, and joins.
- **Model:** reads a configured image column from one parent, preserves all input
  rows and columns, and appends named nullable Float32 SigLIP scores.
- **Result:** converts one terminal Arrow output into a bounded browser preview.

Before accepting an Assistant graph update, `set_workflow` runs the same native
SQL and Model schema planning as Run. An invalid candidate returns its schema
error to the Assistant and leaves the existing graph unchanged. This check does
not consume source batches, load model weights, or start a workflow run.

For example, a Model node is:

```json
{
  "id": "lighting",
  "name": "Score lighting",
  "kind": "model",
  "image_column": "image",
  "features": [
    {"name": "dim_score", "description": "Match to a dimly lit scene", "concept": "a dimly lit scene"}
  ]
}
```

Its parent may be any node that produces the image column. There is no episode-key
contract, source-table ownership requirement, cache relation, or special episode
filter node. Episode decisions are ordinary SQL over scores and source identifiers.
Null images produce null scores; malformed images fail explicitly. SigLIP scores
measure text/image matching, not measured brightness or calibrated probability.

New workflow offers a blank draft or a draft from a dataset. Dataset creation
starts a fresh workflow and conversation with a SQL source reading
`spql.mcap.episodes`; it does not replace the saved workflow currently open.
Rename lives beside the workflow selector and changes the draft until Save.
The Assistant's `inspect_dataset` tool lists datasets when no path is supplied or the path is empty,
or inspects the requested path's actual topics, schema variants, and camera columns.
Dataset recording/import origin does not affect workflow execution.

## Arrow execution

`workflow/execution.rs` plans all node schemas before consuming sources or loading
models. Native ADBC schema planning checks SQL expressions; the Model checks its
image type and output-name collisions. The graph itself does not change during Run.

Eligible adjacent SQL nodes are composed so Spiral can optimize them together.
Shared or repeatedly referenced inputs retain a single Arrow binding rather than
being executed again. Linear operator paths pass record-batch streams. A fork
materializes its output once and shares Arrow buffers between branch cursors. Native
SQL consumes its connection-local temporary Arrow inputs before returning output;
these bindings never enter the project catalog. Large shared image branches therefore
still retain their batches in memory; this implementation does not introduce a disk
spool or a worker pool.

Only the terminal output becomes JSON. The preview's row/byte limits bound the
browser payload; use SQL filters, frame selection, or LIMIT to bound model work.
The runner drains the remaining output so inference finishes and
late failures are not hidden. Progress and cancellation are live observations during
execution. Run admission, progress, cancellation, and terminal snapshots make no
storage writes. Save persists the graph and conversation; run results remain in the
backend project session and are not written into the saved document.

Every Run computes inference again. The old candidate/cache preflight, second
image-producing query, feature backfill, and cache publication are removed from the
runner. Old cache storage/export contracts are not used to execute graphs. Existing
project data is not deleted by this change. Retired graph node shapes are rejected
rather than silently rewritten into a different query.

## Native video and verification

Use `nix develop .#local-driver` to load the locally built release
`target/release/libadbc_driver_spiraldb.dylib` on macOS. Native
`spql.video.from_packets` → `spql.video.frames` → `spql.video.decode` supplies Arrow
`spql.image.Image` columns. Keep packets from different members/cameras separate,
select frames before decoding, and preserve any identifiers needed downstream.

The native integration test runs H.264 decoding → SigLIP → SQL decisions twice,
including duplicate frame selections. It checks scores, input order, unchanged
intent, and absence of published score tables. Other regressions cover generic SQL
fork/join execution, repeated input references, schema failure before execution,
null images, cancellation, read-lease-safe progress, and errors beyond preview limits.
The release driver built from Spiral `xiangpeng/dev` at `fe0726b14` fixes member
pruning across streaming exchanges. The live MCAP `IN (subquery)` and diagnostic
JOIN queries both read only the selected 274 camera packets, down from 10,990.
Native execution measured 3.8 and 5.2 seconds respectively in individual diagnostic
runs. Studio uses the original SQL; the source filtering is handled by Spiral.

## Next milestone: ML reuse

Milestone 3.2 is the immediate follow-up. A planner optimization can push eligible
ML operations into the scanner, read compatible dataset-level derived values,
compute missing selected values, and attach the requested column. Reuse belongs
to the dataset and producer definition, independent of workflow identity. Unsafe
pushdown continues through the ordinary Arrow Model operator. Sparse cache storage,
lineage, invalidation, and missing-value semantics belong to that milestone.

## Assistant tools

Each frontend implementation under `activities/workflow/agent/tools` owns its description, parameter
schema, argument parsing, execution, and receipt. The single registry advertises and
dispatches `inspect_dataset`, `list_relations`, `describe_relations`,
`list_model_capabilities`, `set_workflow`, and `run_query`. There are no preparation,
execution, or separate episode-filter tools. Graph updates use the existing identity,
revision, and catalog-lease checks. Only the user can persist changes with Save.
