# Episode comparison output

Build and Compare are two views of one workflow and its run history. The existing
Assistant authors the measurement SQL and optional episode-comparison output.
The ordinary Run button and `run_workflow` tool execute that complete definition.
Compare has no selection editor, run submission, polling loop, or separate
workflow identity. It reads the selected completed `WorkflowRun`.

## One executable workflow

`WorkflowSpec::execution_sql()` compiles the measurement SQL and configured
selection into one native query. Both EXPLAIN and execution use this same query,
so the existing graph includes the source, model calls, selection, and distribution
aggregates. Ordinary SQL workflows continue to execute their SQL unchanged.
The SQL editor shows the output contract alongside the measurement query.
The Assistant can clear the optional comparison configuration to return to raw
SQL results, including image-evidence queries.

Measurement SQL supplies at most one row per `episode_id`, with numeric columns
for criteria and charts. Frame queries aggregate to episode level first. A native
left join to the dataset episode catalog keeps missing measurements in the base
population. `duration_seconds` and `message_count` come from that catalog. Missing
or nonfinite criterion values remain Unknown. Selected means the criterion
matches, not that training approval has been granted.

The workflow publishes one population summary plus ten equal-width histogram bins
per metric, at most 61 rows for six metrics regardless of episode count. Results
shows these native rows; Compare derives means from their sums and measured
counts. It never calculates dataset counts from a truncated episode preview.
Each episode has equal weight. Mean differences use the metric's units, including
arbitrary model-score units, rather than relative percentages or causal claims.
Duplicate episode rows and truncated aggregates block dashboard presentation.

## Dashboard behavior

- Existing Run publishes the output; Build Results and Compare share run selection.
- The baseline is an implicit select-all workflow by default. Users can instead
  choose another saved workflow, using its latest completed selected population.
  Both outputs keep their own episode counts and means. Changing the baseline
  never runs either workflow.
- Charts normalize by each population's measured count. Missing means stay
  unavailable. Users can add, reorder, and remove metrics.
- Baseline changes and reordering/removing computed metrics reuse the output.
  Changing the selection, measurement SQL, or adding an uncomputed metric marks
  the output stale and requires running the workflow.
- Edit workflow returns to the existing Assistant and graph.
- Chart clicks focus the matching-episode evidence, with 20 episodes per page.
  Inspect recording reuses the synchronized camera/joint workspace.

## Identity and persistence

The backend checks the dataset version before and after execution and records it
on the same run. Evidence queries use the executed workflow definition, checking
that version before and after retrieval. They are native query reads, not new
workflow runs. Evidence still reevaluates the query; no durable episode membership
or inference cache is claimed.

Save persists workflow SQL, comparison configuration, dashboard settings, and
conversation. Runs remain session-owned. A page reload can recover an existing
backend run; a backend restart requires a new run. Cached plans are invalidated
when the complete execution changes, including selection criteria.

## Local demonstration

The saved **Episode comparison — top-camera visibility** workflow uses native
CLIP contrast between dim-scene and well-lit descriptions on the first decoded
top-camera frame. Its provisional criterion is above −3. This is a candidate
ranking, not calibrated illumination. RGB conversion uses the disclosed
`bt601_625_full` assumption. A normal workflow run selects 21 of 52 episodes,
with 31 remaining and no unknowns. Selected mean duration is 7.836 seconds versus
7.113 seconds across the dataset.

**Episode comparison — long recordings** selects durations above 8 seconds:
15 selected, 37 remaining, no unknowns. Both saved definitions use the same
workflow machinery as agent-authored SQL.

Different datasets, dataset versions, measurement definitions, or histogram ranges
are explicitly unsupported for now. Missing baseline metrics are shown as
unavailable rather than silently substituted. Duration and message-count
comparisons work between different selection workflows over the same dataset
version. Custom metrics currently require the same measurement SQL.

Desktop support starts at 1024 CSS pixels.
