# Workflow authoring

A workflow is the unit the engineer creates, names, selects, and returns to.
It owns one executable graph and one Assistant conversation. Graph nodes and edges
are arrays inside JSON, not separate database tables.

## Storage and ownership

The Spiral `workflows` relation stores one row per workflow: `workflow_id`, `name`,
`graph_version`, `graph_json`, `conversation_version`, `conversation_json`, and
`updated_ms`. Graph nodes and edges remain inside the graph JSON. Conversation JSON
contains the visible messages and provider history for one Assistant conversation.

Startup reads IDs and names, ordered latest first, selects the first workflow, and
loads its graph and conversation. An empty list creates a fresh workflow. Switching
loads the selected identity's details; other graphs are not decoded for the picker.

The graph manager owns graph saves; the Assistant owns conversation saves. Each save
updates its part of the row in a transaction, preserving the other part. Saving a
graph also updates its name. Conversation saves require an existing workflow.
`WorkflowJson` pairs each payload with its encoding version, independently of the
internal edit counter used to reject stale requests. New graphs use encoding version
3. Compatible version-2 SQL graphs can still open; retired episode/cache node shapes
are refused without rewriting or deleting their saved payloads.

The graph counter advances only on definition edits. The conversation counter
advances on conversation updates. Selecting a workflow changes neither counter.
Selection touches the graph's storage timestamp, so reconnect opens the latest
workflow. Timestamps are ordered even when two writes occur in the same millisecond.
New workflows receive server-generated identities and an empty graph.

The workspace picker selects the graph and its conversation together. Selection is
serialized with authoring and execution: stop active work and apply or discard
unsaved graph edits before switching. Responses carry server scopes and conversation
identities, so old commands cannot overwrite or cancel work after selection changes.
Canvas selection and layout remain frontend concerns; changing them does not save a
new graph definition. Unsent chat text belongs to its workflow in the frontend.

## Authoring and execution

There is one intent graph and one Run action. The Assistant can inspect datasets,
relations, model capabilities, and bounded read-only results, then save a graph.
It cannot start or wait for a workflow run. Run and Cancel own computation.

The graph has four node kinds:

- **Table:** shorthand for a SQL source over a qualified relation.
- **Query:** a SQL source with no incoming edges, or a transform over connected
  `{{node:id}}` inputs. SQL owns filtering, sampling, projection, aggregation, and joins.
- **Model:** reads a configured image column from one parent, preserves all input
  rows and columns, and appends named nullable Float32 SigLIP scores.
- **Result:** converts one terminal Arrow output into a bounded browser preview.

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

The dataset picker starts a graph with a SQL source reading `spql.mcap.episodes`.
The Assistant's `inspect_dataset` tool lists datasets when no path is supplied,
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
execution. The terminal snapshot is saved after streams release their catalog read
leases, avoiding progress writes that would block downstream reads. On interruption,
the persisted in-progress run is recovered as interrupted.

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

Each implementation under `workflow/agent/tools` owns its description, parameter
schema, argument parsing, execution, and receipt. The single registry advertises and
dispatches `inspect_dataset`, `list_relations`, `describe_relations`,
`list_model_capabilities`, `set_workflow`, and `run_query`. There are no preparation,
execution, or separate episode-filter tools. Graph saves use the existing identity,
revision, and catalog-lease checks.
