# Workflow comparison

Workflow → Compare compares two saved workflows. Workflow B defaults to **All episodes** in the dataset identified by the comparison agent.

1. Build and save any ordinary workflow with the existing Assistant, SQL editor, graph, and Run controls.
2. Open Compare, select Workflow A and Workflow B, and click **Compare**.
3. The comparison agent reads saved SQL, output schemas, small previews, task context, and recorded topic schemas and sensor metadata. It submits one complete dashboard plan, including population normalization and all measurement cards.
4. Spiral establishes the complete populations, then runs up to four planned card analyses concurrently, starting the next queued card whenever one finishes. Each card appears as its query finishes, with means, differences, measurement coverage and distributions using shared bin boundaries. A failed card does not discard completed results or stop other queries.
5. Open **Metrics** to show or hide the agent-discovered metrics without rerunning the analysis. Episode populations and histogram bins link to the existing recording inspector.

The user sees preparation status and the resulting dashboard, not the agent's reasoning. Comparison never edits either workflow or its conversation.

## Ownership

Workflows contain ordinary SQL and conversation history. They have no comparison metadata, mandatory episode output format, or selection contract. The recent experimental contract was removed without a runtime compatibility layer.

`ComparisonPlan` owns the temporary interpretation: two normalization queries over fixed source CTEs, an observation unit, optional dataset, and up to eight numeric metrics. Episode normalizers must return one non-null unique `episode_id` per selected episode; repeated frame or moment rows must first be grouped. Generic tabular outputs can be compared as rows. Incompatible outputs produce a short explanation.

The compiler embeds source SQL unchanged. Each recorded-signal card computes one value per episode and LEFT JOINs it into both fixed populations, preserving episodes with missing measurements. It computes exact counts and ten shared bins per metric over the complete results. Every card checks the dataset version and population sizes against the initial comparison. Missing and non-finite metric values are excluded from measured counts and averages. Population counts remain independent: neither workflow must be a subset of the other.

The all-episodes baseline uses the identified dataset's native episode catalog. Inspection uses the same source SQL and normalization snapshot as the dashboard, with dataset version checks around both aggregation and evidence loading. A changed dataset requires a new comparison.

## Physical measurements

Cards can measure scalar fields with native SQL, or inspected array fields with a typed query compiler. The agent chooses fields, aggregation, actual named joints, validity flags and units from recorded schemas and metadata. It is instructed to favor diverse measurements such as motion range, gripper posture, tracking error, temperature, voltage and load over repeated volume counters. Configuration limits are not readings; uncertain units and suspicious raw values are disclosed.

The demo uses the explicitly approved SQL workaround for Spiral’s UUID/UNNEST limitation: validate a one-to-one mapping between recorded textual episode keys and native IDs, expand only text keys and arrays, aggregate by textual key, then restore native UUIDs. Named joint arrays use the recorded metadata names. Missing or ambiguous identity mappings fail explicitly.

## Boundaries

Comparison currently reevaluates saved SQL; it does not reuse materialized workflow results. Preview limits bound returned rows, not native computation. Results and panel choices live in the current comparison workspace and are not saved dashboards. Manual selection shows or hides discovered metrics; defining arbitrary new metrics remains future work.

## Verification

Shared tests cover unchanged source queries, read-only SQL, shared distributions, required dataset identity for the default baseline, episode identity validation, and inspection query consistency. Browser tests cover independent population counts, measured means, missing values, and rejection of truncated or duplicate-identity results.

Live demo verification (2026-09-15): the ordinary toy-side decision workflow was interpreted as 49 retained episodes versus the default 52-episode population, with mean durations 7.227s and 7.113s. Comparing it to the ordinary long-recordings workflow produced 49 versus 15 episodes and mean durations 7.227s versus 9.943s. Metric toggles updated immediately; episode evidence and the existing recording inspector loaded. Both saved documents, including conversation histories, were unchanged by comparison. The two recent experimental demo records were updated once to ordinary SQL before removing the old contract; there is no runtime migration layer.

The comprehensive batch was verified live against 15 long recordings and 52 baseline episodes. Ten cards included temperature, voltage, load, current, tracking error and named-joint motion. A capture shows five cards complete while five are still running. Follower maximum recorded peak temperature averaged 106.9°C versus 89.2°C; the recording contains suspicious 150°C readings, which remain disclosed as recorded. Out-of-order completion tests verify independent pending/error state and preservation of manual metric visibility.
