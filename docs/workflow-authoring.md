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
internal edit counter used to reject stale requests.

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

The Assistant can inspect relations, inspect model capabilities, issue bounded
read-only queries, and save the graph. It cannot start or wait for workflow runs.
Even an unadvertised call to the old run tool is rejected in the backend. The
existing explicit Run and Cancel controls still own computation.

SQL nodes can read a dataset or table directly, or transform named upstream inputs
using `{{node:id}}`. They do not require a Table node. `WorkflowSqlNode` uses native
ADBC schema planning and accepts named Arrow batch streams for filters, projections,
aggregates, and joins. Input bindings are connection-local temporary relations.
Schema errors are rejected before those streams are consumed. Connected SQL queries
in the runner remain composed as subqueries so Spiral can optimize them together;
only terminal results become bounded JSON previews.

The local driver supports `spql.video.from_packets` → `spql.video.frames` →
`spql.video.decode`, including Arrow `spql.image.Image` output. Run Studio with
`nix develop .#local-driver` to use `target/release/libadbc_driver_spiraldb.dylib`
on macOS. The native integration test shares the playback H.264 fixture and checks
frame selection and decoding through that driver. The ML cache and typed
episode-filter graph still need to be moved onto the generic Arrow node contract.

There is no conversation picker inside a workflow, graph revision browser, restore
operation, or duplicated filter-interpretation object. New graph saves replace the
current definition instead of collecting an edit history. M3 will pin the executed
definition with prepared output.

## Assistant tools

Each tool implements `WorkflowTool` under `workflow/agent/tools`: its provider name,
description, parameter schema, typed argument parsing, call, availability, and
public presentation belong together. The registry lists each implementation once.
Provider advertisement and backend dispatch use the same availability rule;
unknown names have no executable implementation. There are no run or wait tools.

Graph saving still goes through the manager's existing validation, identity,
revision, and catalog-lease checks. The registry does not implement another save
path. Progress labels and receipts also come from the tool implementation rather
than separate name matches. The graph models and their current capabilities are
unchanged by this refactor.

Verification: 281 backend tests and strict Clippy passed. Tool tests execute the
registered implementations, check malformed arguments before storage or mutation,
and verify that dispatch rejects every tool excluded from advertisement.

## Native episode filters

Choose a native dataset from the workflow header, then describe which episodes to
keep. Selection saves a Dataset node containing the real dataset resource/version
identity. Choosing a different dataset clears the old filter because its camera
bindings belong to the previous source.

The Assistant uses the existing dataset catalog and native Arrow schema inspection
to discover encoded-video topics. A supported camera must have the actual
`foxglove.CompressedVideo` schema and binary `value.data` plus string `value.format`
fields. The server validates the dataset identity and camera binding again before
saving; invented cameras or changed source versions are rejected.

The graph JSON holds four typed nodes:

`Dataset → Camera scoring → Episode filter → Result`

Camera scoring records the discovered topic/schema binding, the supported
`siglip-base` producer, a visual text concept, and a sampling interval. The episode
rule records the score threshold, whether to exclude on any matching sample or
more than a specified percentage, and whether unknown episodes remain in the
output. There is no separate interpretation record or new storage table. Corrections
replace these node configurations while preserving workflow and node identities.

Sampling means the first frame in each interval measured from episode start,
including the final partial interval. A missing camera, empty interval, invalid
score, or failed score makes the episode unknown. Unknown remains a distinct
evidence decision even if the user chooses to keep it in the output. Thresholds
and episode fractions use strict greater-than comparisons. The fraction counts
sampled frames, rather than claiming exact dark duration.

For dim lighting, the Assistant defaults to a dim-scene SigLIP concept, one-second
sampling, a score above 0.5, more than 50% matching samples, and excluding unknowns.
These defaults are provisional: SigLIP measures text/image similarity, not physical
brightness. The Assistant explains these choices and can adjust them in plain
language. The Results panel derives its mechanical explanation and required
inputs directly from the graph, alongside **Needs preparation**.

Native authoring exposes only dataset/schema inspection, supported model
capabilities, and typed filter edits. Neither raw SQL nor execution tools are
available on this path. The backend also rejects explicit Run requests for native
drafts before entering the relation runner. This typed draft is not yet connected
to the SQL/Arrow execution path. Native packet decoding is available in the local
driver; the remaining work is connecting image inputs, reusable ML outputs, and
SQL episode evaluation under one Run action.

## Verification

Focused tests cover conversation identity, stale turns, restart repair of incomplete
tool exchanges, malformed history, workflow selection, and rejection of old run tools.
The hosted `spiral_sql_workflows_persist_and_cancel` regression uses disposable
relations to verify execution, named workflow saves and selection, separate
conversations, stale commands, and reconnect to the latest workflow, then removes
its test relations. Browser tests cover the shared frontend state boundaries.

Native filter tests cover camera/sampling/threshold validation, unknown decisions,
strict threshold boundaries, corrections with stable identities, JSON reopen, and
the authoring tool and execution boundaries. Against `dataset_default` (52 native
members), direct API verification discovered the top and gripper cameras, saved a
filter, rejected a nonexistent camera without altering the draft, applied two
corrections, and confirmed that Run stops before execution. The desktop UI was
checked at 1280 and 1024 pixels, including internal Results scrolling.

On September 10, 2026, the approved live Assistant test through OpenRouter
(`openai/gpt-5.6-terra`) authored the real top-camera lighting instruction with no
parameter questionnaire. “Use the wrist camera instead” selected the actual gripper
topic without changing the rule. A second correction changed majority aggregation
to any dark sample without changing the camera. An absent rear-camera request
listed the two real cameras and left the graph unchanged. All turns used only
metadata inspection, capability discovery, and typed graph saves; no model work
ran. The graph and conversation survived a Studio restart together.

Validation: 436 native Rust tests and 122 headless browser tests passed. Native
Clippy passed; WASM Clippy passed with the existing `suspicious_else_formatting`
lint in the unrelated Rollout Dioxus markup excluded. The bounded desktop finish
review returned **ship**.

After restoring M2.1 onto the dataset-boundary and recording-lifecycle cleanup,
the combined tree passed 436 native Rust tests, 122 browser tests, and the same
Clippy checks. Restarting Studio reopened the identical four-node graph and
eight-message Assistant conversation, including its identity and revision,
without starting workflow execution. This verification reused the saved live-test
draft and made no additional provider calls.
