# Workflow authoring

A workflow is the unit the engineer creates, names, selects, and returns to.
It owns one executable graph and one Assistant conversation. Graph nodes and edges
are arrays inside JSON, not separate database tables.

## Storage and ownership

The existing Spiral `workflow_runs` relation stores independently versioned JSON
payloads under `workflow:<id>` and `assistant:<id>`. The graph manager owns the
current graph and its latest execution observation. The Assistant owns its visible
messages and provider tool history. Each save replaces only its own key inside the
existing transaction, preserving every other workflow and conversation.

This split follows the independent write patterns: streaming and tool completions
must not overwrite a graph. It does not introduce a relational message schema or a
second graph representation. `WorkflowJson` describes the payload encoding version;
it is distinct from the internal edit counter used to reject stale requests.

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

## Existing projects

The old `default` and `agent:default` rows are read as legacy data. The current graph
and selected legacy conversation open normally. Subsequent saves use the new keys;
the original legacy rows remain intact, including graph revisions and inactive
conversations, and remain part of project exports. Inactive legacy conversations
are not exposed as separate workflows because their exact associated graph was
never recorded. Unsupported or malformed history is refused instead of reset.

## Verification

Focused tests cover conversation identity, stale turns, restart repair of incomplete
tool exchanges, malformed history, legacy selection, and rejection of old run tools.
The hosted `spiral_sql_workflow_runs_persists_and_cancels` regression uses disposable
relations to verify execution, named workflow saves and selection, separate
conversations, stale commands, and reconnect to the latest workflow, then removes
its test relations. Browser tests cover the shared frontend state boundaries.
