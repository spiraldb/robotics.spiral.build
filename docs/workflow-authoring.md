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

There is no conversation picker inside a workflow, graph revision browser, restore
operation, or duplicated filter-interpretation object. New graph saves replace the
current definition instead of collecting an edit history. M2 still needs native
episode-filter semantics; M3 will pin the executed definition with prepared output.
This simplification does not claim that native lighting preparation is implemented.

## Verification

Focused tests cover conversation identity, stale turns, restart repair of incomplete
tool exchanges, malformed history, workflow selection, and rejection of old run tools.
The hosted `spiral_sql_workflows_persist_and_cancel` regression uses disposable
relations to verify execution, named workflow saves and selection, separate
conversations, stale commands, and reconnect to the latest workflow, then removes
its test relations. Browser tests cover the shared frontend state boundaries.
