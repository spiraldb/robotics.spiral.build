# Workflow comparison

Workflow → Compare compares two saved workflows. Workflow B defaults to **All episodes** in the dataset identified by the comparison agent.

1. Build and save any ordinary workflow with the existing Assistant, SQL editor, graph, and Run controls.
2. Open Compare, select Workflow A and Workflow B, and click **Compare**.
3. The comparison agent reads saved SQL, output schemas, small previews, task context, and recorded topic schemas and sensor metadata. It submits one complete dashboard plan, including population normalization and all measurement cards.
4. Spiral establishes the complete populations, then runs up to four planned card analyses concurrently, starting the next queued card whenever one finishes. Each card appears as its query finishes, with means, differences, measurement coverage and distributions using shared bin boundaries. A failed card does not discard completed results or stop other queries.
5. Open **Metrics** to show or hide the agent-discovered metrics without rerunning the analysis. Episode populations and histogram bins link to the existing recording inspector.

The user sees preparation status and the resulting dashboard, not the agent's reasoning. Comparison never edits either workflow or its conversation.

While analysis runs, a four-stage progress panel explains running workflows,
choosing measurements, comparing populations, and calculating measurements. It
shows elapsed time and real ready/running/queued/unavailable card counts after
the plan is known. Planned cards appear before population analysis completes;
their placeholders distinguish queued work from active calculations. The panel
and timer unmount as soon as analysis ends, leaving results and any errors without
a completion banner. Progress accounting, independent completions, failure
retention and reset behavior are covered by the frontend comparison tests.

## Ownership

Workflows contain ordinary SQL and conversation history. They have no comparison metadata, mandatory episode output format, or selection contract. The recent experimental contract was removed without a runtime compatibility layer.

`ComparisonPlan` owns the temporary interpretation: two normalization queries over fixed source CTEs, an observation unit, optional dataset, and up to eight numeric metrics. Episode normalizers must return one non-null unique `episode_id` per selected episode; repeated frame or moment rows must first be grouped. Generic tabular outputs can be compared as rows. Incompatible outputs produce a short explanation.

Each comparison captures both complete workflow outputs once and binds them as native temporary Arrow inputs for every card. The compiler embeds those input references instead of re-executing source SQL. Each recorded-signal card computes one value per episode and LEFT JOINs it into both fixed populations, preserving episodes with missing measurements. It computes exact counts and ten shared bins per metric over the complete results. Every card checks the dataset version and population sizes against the initial comparison. Missing and non-finite metric values are excluded from measured counts and averages. Population counts remain independent: neither workflow must be a subset of the other.

The all-episodes baseline uses the identified dataset's native episode catalog. Inspection uses the same captured outputs and normalization as the dashboard, with dataset version checks around both aggregation and evidence loading. A changed dataset requires a new comparison.

## Physical measurements

Cards can measure scalar fields with native SQL, or inspected array fields with a typed query compiler. The agent chooses fields, aggregation, actual named joints, validity flags and units from recorded schemas and metadata. It is instructed to favor diverse measurements such as motion range, gripper posture, tracking error, temperature, voltage and load over repeated volume counters. Configuration limits are not readings; uncertain units and suspicious raw values are disclosed.

Array queries preserve native episode UUIDs through UNNEST, aggregate by UUID, and join joint-name metadata by the same UUID. They do not depend on textual episode labels. Named-joint metadata must have one row per native episode.

## Boundaries

Each selected workflow executes once at the start of a comparison. Its complete typed Arrow output stays in memory and is reused by the population query, all cards, and episode evidence. Starting another comparison runs the workflows again. Nothing is persisted on the workflow row and there is no database schema change. Native temporary inputs use independent connections so up to four card queries can run concurrently. Native column and extension metadata are retained; joining reloaded generic UUIDs to UUIDv7 requires compatible upstream coercion.

Preview limits bound returned rows, not native computation. Dataset inventories are prefetched alongside source capture and included in the first agent request. Cards execute directly without an unused full EXPLAIN plan. Captured pairs are shared by cards and episode evidence and released when their workspace closes, or at project-session shutdown. Results and panel choices live in the current comparison workspace and are not saved dashboards. Manual selection shows or hides discovered metrics; defining arbitrary new metrics remains future work.

## Verification

Shared tests cover unchanged source queries, read-only SQL, shared distributions, required dataset identity for the default baseline, episode identity validation, and inspection query consistency. Browser tests cover independent population counts, measured means, missing values, and rejection of truncated or duplicate-identity results.

Live demo verification (2026-09-15): the ordinary toy-side decision workflow was interpreted as 49 retained episodes versus the default 52-episode population, with mean durations 7.227s and 7.113s. Comparing it to the ordinary long-recordings workflow produced 49 versus 15 episodes and mean durations 7.227s versus 9.943s. Metric toggles updated immediately; episode evidence and the existing recording inspector loaded. Both saved documents, including conversation histories, were unchanged by comparison. The two recent experimental demo records were updated once to ordinary SQL before removing the old contract; there is no runtime migration layer.

The comprehensive batch was verified live against 15 long recordings and 52 baseline episodes. Ten cards included temperature, voltage, load, current, tracking error and named-joint motion. A capture shows five cards complete while five are still running. Follower maximum recorded peak temperature averaged 106.9°C versus 89.2°C; the recording contains suspicious 150°C readings, which remain disclosed as recorded. Out-of-order completion tests verify independent pending/error state and preservation of manual metric visibility.

Native UUID verification (2026-09-15): rebuilt the local ADBC driver from Spiral commit `23c8296a6`, which includes the UNNEST fix `6e9d60e6a`. Temperature and named-joint motion queries each returned 52 distinct native UUIDs without textual-label mapping. The complete temperature comparison matched every previously verified distribution row exactly.

In-memory reuse verification (2026-09-15): the backend test captures 150 rows with an eight-row preview, then executes simultaneous aggregates and a two-input join over the full output. Separate comparisons are isolated and release invalidates only the released pair. All 25 backend workflow tests and three frontend comparison tests pass. Database schema and persistence code are unchanged.

Latest local driver verification (2026-09-15): rebuilt the user's updated local patch on `23c8296a6` (library SHA-256 `c133a1ac5c5aa86a75b132c9aae432d9cfea6e505653e9116148971793ad5bb7`). Both UUID joins and UNION ALL now pass. Two complete browser comparisons produced 49 selected versus 52 baseline episodes and five successful cards, with the first card ready in 38.8/34.9 seconds and all cards ready in 45.708/39.121 seconds. Physical cards ran four at a time. The comparison tool contract explicitly requires every initial metric column and reuse of captured workflow CTEs. No SQL cast workaround is installed. See [the comparison profile](comparison-profile.md) for the complete timings and earlier diagnostic history.
