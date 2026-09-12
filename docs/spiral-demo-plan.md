# Spiral robotics demo plan

Status: **SQL workflow cutover implemented; adapter and browser checks passing**, September 11,
2026. Native scoring and EXPLAIN are available in the current local upstream driver. Studio now authors and runs
SQL and displays Spiral's structured JSON plans with Dioxus Flow. The logical plan
is the default; optimized logical and physical views are also inspectable.
Workflow cancellation is out of scope. Native reuse and durable outputs remain
future milestones.

Implementation is organized in [the milestones](spiral-demo-milestones.md).
Each task has a Done checkbox for tracking completion.

## 1. Goal and agreed direction

An ML engineer has a dataset already loaded into Spiral. The dataset contains
multiple episodes; each episode originated as one MCAP file containing multiple
camera feeds and joint samples. Studio should let the engineer inspect those
recordings, describe which episodes to keep, run the SQL query that computes the
selection, and use the workflow as an input to training.

The central demonstration is:

> Inspect the dataset. Ask to exclude episodes whose top camera is too dim.
> Let the agent write SQL and inspect its query plan. Click Run, review the
> selected episodes, then train from the completed output without recomputing
> its ML predicates.

An optional second condition is “also exclude episodes where the ball was not
placed in the bowl.” This exercises a pipeline with object detection or another
appropriate visual model, followed by an episode-level decision.

The following are requirements from the discussion:

- **Data** exposes episodes, their multiple camera feeds, and joint samples for
  synchronized inspection. Import has already happened before this flow starts.
- **Workflow** starts with human-written instructions and generates SQL that selects
  whole episodes and may add derived outputs. SQL is the
  executable definition; the displayed graph comes from Spiral's query plan.
- **Spiral** owns native AI operations, query planning, optimization, and execution.
  Studio does not own executable nodes, edge routing, or intermediate Arrow batches.
- **The workflow agent** interprets vague instructions and chooses concrete,
  executable semantics. Optional follow-up questions can help refine the workflow;
  users are not required to specify every parameter or resolve every ambiguity.
- **Run** is the single explicit computation action. Spiral evaluates the query,
  including inference. Native reuse and durable completed outputs follow the first
  working SQL execution; there is no separate preparation stage.
- **Training** can choose a workflow as its data source. Its input path consumes
  completed outputs and never evaluates the workflow's enrichment models on the fly.

Proposed product headline: **Describe the episodes you want, inspect the plan,
and run it to get a dataset you can review and train from.**

The [Spiral homepage](https://spiraldb.com/) was a presentation reference in the
first draft. The episode → SQL workflow → Run → review → training flow above now owns
the design. Website examples are not evidence of installed driver capabilities.

## 2. Product model

There are three user-facing objects, with revisions underneath them:

| Object | Meaning |
| --- | --- |
| Dataset | The source collection of MCAP episodes already in Spiral, with camera and joint streams. Source recordings remain available after filtering. |
| Workflow | A named document containing SQL and one Assistant conversation, with a document revision. SQL expresses sources, model operations, episode predicates, and output fields. The plan visualization is derived, not another executable definition. |
| Completed workflow output | The result of applying an exact submitted SQL definition to a specific source snapshot: persisted derived values, coverage, and resolved episode membership. Inspection and Training consume this same result. |

“Remove episodes” means exclude them from the workflow output. It does not delete
source MCAP files or source episodes. “Augment the dataset” means expose persistent
derived columns alongside the original data through native Spiral relations;
physical persistence must use verified Spiral capabilities. It does not require
rewriting the MCAP files or copying every recording into a new dataset.

The full-dataset milestone produces a durable result in addition to any native
inference cache. A completed output records the source snapshot, exact submitted
definition, producer definitions, coverage, and selected episode identities. Model-derived outputs may serve only
as filter evidence, or may also be exposed for downstream use. A training pipeline
must explicitly map any derived field it intends to consume.

Whole episodes are the selection unit for this demo. Individual frames and time
ranges provide evidence for a decision. Training windows are sampled later from
selected episodes; selecting an episode does not make every possible window valid.
Interval curation is a separate future extension.

## 3. Data view: understand an episode

The initial dataset view shows the actual episode population and available
streams. Opening an episode exposes all recorded camera feeds, joint state traces,
and recorded actions when present, using one shared Timeline. Camera roles such
as “top” must map to actual recorded topics, visibly and consistently across the
dataset. Live hardware configuration must not supply missing recorded streams.

The engineer can play, pause, scrub, compare cameras, and inspect joint samples
at the same time. Preserve source timestamp precision and establish clock mappings;
never align independent streams by row index. Show missing intervals, unavailable
feeds, decode errors, and absent action channels explicitly.

From Workflow results, **Inspect episode** returns to this same view with the
completed output reference, episode identity, and relevant evidence time. It can
show a derived value or detection overlay when that output exists, while keeping
the original recordings available for comparison. Evidence timestamps are not
selection boundaries: an excluded episode is excluded as a whole.

Start by auditing the current archive for actual camera roles, joint/action
channels, and visible task variation. The dim-camera example is the primary story;
the bowl example depends on suitable recordings and a validated producer.
An archive with only joint states can still be inspected, but action-target
availability must be resolved before promising policy training.

## 4. Workflow view: instructions become SQL

The engineer selects an existing Spiral dataset and types:

> Select all episodes, but remove the ones whose top camera is too dim.

The Assistant interprets the request and edits the in-memory SQL draft with a
readable explanation. Natural language is the primary authoring path for this
demo; SQL remains directly inspectable and editable. Save
persists the definition and conversation together. Run submits an immutable copy
of the visible SQL, including when the draft has not been saved.
Neither action silently performs the other.

Spiral validates the query and returns its plan without executing inference.
Studio renders the native structured logical plan with Dioxus Flow by default.
The plan selector also exposes optimized logical and physical graphs; selecting
a node shows native schemas and properties.
Only metrics actually exposed by Spiral may be shown. Named CTEs can explain stages, but
are not materialization promises or stable execution-node identities. The engine
may inline, fuse, duplicate, or reorder operations only when their semantics allow
it. Editing SQL regenerates the plan; this demo does not introduce an independently
editable graph or bidirectional SQL/graph synchronization.

The draft identifies:

- The source dataset and population to process.
- Which camera topic represents the top camera.
- Required derived columns, their producers, and their output grain.
- How frame-level observations become episode-level values.
- The episode predicate and fields exposed by the output.

The ownership and data flow are:

```text
Instruction → Assistant → SQL
                            ├─ Spiral planning → JSON plan → Dioxus Flow
                            └─ Run → Spiral query execution → results
                                       ├─ native decoding and inference
                                       └─ native reuse and materialization (later milestones)
```

The SQL can express camera/frame selection, decoding, model evaluation, episode
aggregation, and inclusion decisions in one query. The initial native AI
catalog includes CLIP image/concept scoring through `spql.ai.score`; its logits
are not equivalent to the former Studio SigLIP scores. Actual function
names and signatures must come from Spiral's implemented catalog; examples and
agent suggestions must not invent supported APIs. Deterministic calculations can
also serve as producers. Choose them for the requested meaning: image/concept
matching is not measured brightness or calibrated probability, and object detection
alone does not establish that a ball was placed in a bowl.

### Execution ownership and native requirements

| Owner | Responsibilities |
| --- | --- |
| Spiral | SQL parsing and type/schema validation; logical and physical plans; native image/AI functions or operators; optimization, batch execution, model resources; later, compatible reuse and atomic output materialization. |
| Studio | Workflow documents, conversations, explicit Run, query submission and observation, plan rendering, bounded result display, and navigation to evidence or Training. |
| Training runtime | Training-job lifecycle, pinned input loading, sampling, optimization, and checkpoints. Physical robot actions retain their separate lifecycle APIs. |

Implement native inference as part of Spiral's execution system, reusing the
existing model behavior where appropriate. A SQL function name alone is insufficient:
define typed image inputs/outputs, null/error behavior, model and preprocessing
versions, batching/resource ownership, and determinism or volatility. Specify when
an AI expression may be eliminated, moved, duplicated, or reused; CTE syntax alone
must not be used to guarantee one evaluation. Native planning must not load weights
or run inference. Consume native EXPLAIN JSON schema version 2;
Studio must not parse display text into a second planner or synthesize node progress.

Arrow may remain the query-result transport and an engine implementation detail.
Remove Studio's graph scheduling, SQL-node fusion, node placeholders, temporary
relations for graph edges, fork materialization, and external Model operator after
the SQL cutover. Reuse the existing query connection, run-observation, document,
and result-display components; do not build another scheduler around native SQL.

Authoring can inspect schemas, capabilities, and bounded source data. Enforce the
computation boundary in the backend: a read-only SELECT can still invoke AI or
perform expensive work. Authoring query inspection must not provide a bypass for
workflow inference; use native function/capability information, including through
views, rather than assuming read-only SQL is sufficient authorization.

Resolving ambiguity is part of the agent's job. Using the instruction, dataset
schema, available evidence, supported producers, and conversation history, it
chooses camera mappings, scoring methods, thresholds, aggregation rules, temporal
meaning, and unknown-value handling. These are workflow-specific decisions, not
product-wide definitions that the human must settle before authoring can work.

The agent records its choices in the executable definition and explains the
consequential ones in plain language. The user can inspect or correct them without
approving each parameter. An agent-selected threshold is a heuristic unless
calibration evidence exists; interpretation must not become a fabricated claim
about data or model quality. Unsupported producers remain unresolved dependencies,
not executable capabilities. Generating or editing a draft must not silently
launch the full dataset enrichment job.

### Examples of agent interpretation

These examples illustrate decisions the agent can make. They do not prescribe
fixed semantics or require the user to choose them in advance:

| Request | Example executable interpretation | Decisions owned by the agent |
| --- | --- | --- |
| “Top camera is too dim” | Derive a frame-level lighting value, then an episode summary such as the fraction of sampled frames below a threshold. Exclude the episode when that fraction exceeds the chosen allowance. | Actual camera mapping; measured brightness versus learned visibility; sampling cadence; value scale; darkness threshold; allowed duration/fraction; minimum coverage. |
| “Ball was not placed in the bowl” | Assess whether the ball remains in the bowl over a terminal observation interval. Keep only episodes assessed as successful when this condition is enabled. | Final state versus ever observed inside; camera; model; terminal interval; evidence/confidence requirements; occlusion handling. |

For the bowl request, the agent may interpret success as a final state or as an
event observed during the episode, depending on context. It should explain the
chosen meaning, and can ask a follow-up if that distinction would materially
change the intended output. Similarly, it chooses lighting thresholds and sampling
rules using available evidence. Explicit sample previews can help refine these
choices; an image/concept score must not be described as a calibrated success
probability without validation.

Keep not-yet-computed, computed-but-unknown, and computed true/false distinct.
Missing or occluded evidence must not become a positive success result or a negative
lighting assessment by default. A suggested starting policy is to exclude unknown
episodes from trainable membership and list them separately for review. The agent
chooses the policy for the request, makes it visible in the workflow definition,
and includes it in the revision. Any decision to include unknowns must preserve
their unknown classification rather than relabel them as successful assessments.

### Optional follow-up questions

An optional authoring enhancement lets the agent ask focused questions within the
Workflow Assistant when an answer would materially improve its interpretation.
Clarification is a tool the agent can choose, not a mandatory form or approval
step before every generated workflow. When context supports a reasonable choice,
the agent proceeds and makes its interpretation inspectable.

Proposed interaction:

- The agent asks a short question tied to the current instruction and explains
  the consequence: “Should the ball still be in the bowl at the end, or does
  entering it at any point count?”
- Show a small set of suggested answers and allow a free-text response. When a
  reasonable default exists, offer **Use your judgment** so the user can delegate
  the choice explicitly. A highlighted suggestion is not a submitted answer.
- Keep the generated draft visible while clarification is pending. The agent
  can continue independent work; only the parts that depend on a required answer
  wait. Distinguish a missing required fact, such as an unresolvable camera role,
  from an optional preference the agent can reasonably decide.
- Apply the answer to the same workflow draft, update its explanation and
  executable definition together, and show what changed. Preserve the exchange
  so later edits have the same context.
- If the answer changes an already executed workflow, revise its SQL and explain
  the expected work on the next Run. Spiral owns reuse and invalidation. Asking
  or answering a question does not execute the query.

Without this enhancement, the agent still makes reasonable semantic decisions
and explains them; users can refine the draft through ordinary instructions.
Genuinely missing required inputs remain explicit blockers. Follow-up UI is
optional scope, while agent ownership of interpretation is part of the core flow.

## 5. Run the query and consume its output

There is one Run action, whether values are cached or must be computed. The
workflow is the saved query; a plan explains how Spiral intends to evaluate it.
A completed output is a separate result tied to the exact submitted definition
and source snapshot. Saving or editing a workflow does not mutate a running query
or an earlier result.

### First delivery: native SQL execution

Start with one episode: select and decode frames, score them, and aggregate an
episode decision entirely inside Spiral. Studio submits the query and displays
its result. Run observations and bounded previews can remain session-owned in
this delivery. Each Run computes inference again; no persistent reuse or trainable
output is claimed yet. A preview row limit bounds display, not necessarily query
work. Use explicit SQL selection to bound computation and report terminal query
success or failure independently of whether a preview is already visible.

### Full demo: reusable values and durable results

After native reuse and full-dataset materialization land, Run:

1. Captures the submitted SQL and document revision or unsaved
   definition, then pins the source snapshot through native Spiral support.
2. Uses Spiral's planner and engine to validate and execute the query, computing
   selected missing values and reusing compatible values when allowed.
3. Reports actual engine progress and errors, without inventing operator progress.
4. Atomically publishes complete episode membership, decisions, evidence, and
   coverage with source/query/producer provenance after successful execution.
5. Makes that completed output available to inspection and Training.

Native result materialization must not run the expensive query a second time just
to publish its previewed result. There is no separate Prepare or Publish action.
An incomplete or failed run cannot appear as a completed training input. Native
inference-cache commits and completed workflow outputs have separate lifetimes;
reusable partial cache work does not imply a complete output.

Opening results, inspecting their decisions, and starting training consume the
completed output without reevaluating enrichment. Training pins exact membership
rather than resolving a moving query per batch. Original media decoding and
training-time augmentation remain ordinary data loading operations.

### Operational states

| State | Visible meaning and action | Training behavior |
| --- | --- | --- |
| Draft / changed | Inspect SQL and plan; **Run** the visible definition. | Explicitly choose an earlier completed output, if available. |
| Running | Show available native progress; cancellation is out of scope. | Cannot use the in-progress result. |
| Completed | Show results; **Use in Training** is available only for a durable compatible output. | Pin that output and its source dependencies. |
| Failed | Show the native error and allow another **Run**. | Cannot use an incomplete result. |
| Source changed | Offer **Run** against the new snapshot; identify the previous result as older. | Earlier completed outputs keep their pinned inputs. |

A successful result can contain computed unknowns under its declared policy;
unexpected execution/decode failures must not silently become ordinary unknowns.
A disconnected client is not evidence that a query succeeded or failed. Reconnect
recovery and durable output publication belong to the full-dataset milestone.

### Reuse and changes

| Change | Required work on Run, after native reuse lands |
| --- | --- |
| Same source and producer semantics | Reuse compatible model values. |
| Threshold over a stored score/summary | Reevaluate the SQL selection; publish a new output without model inference. |
| Aggregation with sufficient stored observations | Recompute the summary and selection from those observations. |
| Model, concept prompt, preprocessing, camera, or sampling change | Compute affected selected values unless compatible results exist. |
| Additional model condition | Compute its missing values and reuse compatible existing values. |
| Added or modified episodes | Run against a new snapshot; reuse only provably compatible inputs. |

Spiral owns these optimizations, including safe scanner pushdown. When pushdown
is unsafe, ordinary native AI execution remains inside Spiral. Reuse identity
includes source content/version and producer semantics, not workflow IDs or column
names. Not-yet-computed, computed null/unknown, and computed values remain distinct.
No Studio dependency planner or cache preflight is introduced.

## 6. Training view: choose a workflow as input

In Training, the engineer chooses **Workflow** as the data source, then a saved
workflow and completed output revision. **Use in Training** from Workflow opens the
same configuration with those references already filled in.

Before **Start training**, show:

- Workflow name/revision, completed output reference, and source snapshot.
- Selected episode count and separately the count of valid training windows.
- Camera, joint-state, and action mappings required by the selected training pipeline.
- Optional derived output mappings, only when supported by that pipeline.
- Sampling/context/action horizon, split policy, and normalization configuration.

A completed workflow output is not automatically training-compatible. Missing action
channels, incompatible shapes, insufficient valid windows, or unavailable runtime
must have specific explanations. If a completed output is required, link back to
that workflow and its **Run** action. Starting training must not trigger
an implicit enrichment job or substitute the unfiltered source dataset.

For the initial training proof, use the existing ACT lifecycle with one workflow
output. Derived columns can serve purely as selection evidence; feeding detections
or other new tensors into ACT is a separate capability decision. Filter outputs
are not automatically policy inputs or targets.

Split by whole episode before sampling windows. Keep train/evaluation membership
disjoint, freeze evaluation membership before iterative rule tuning for any quality
comparison, and derive normalization statistics only from training data. Windows
must remain within a single selected episode and valid synchronized coverage;
reject windows crossing gaps. Record sampler configuration and any exclusions
introduced by training compatibility separately from workflow exclusions.

A training run pins the completed output, loader configuration, and resulting input
membership. Its checkpoint links back to those references. Changing a workflow
later does not change that run. The demo proves a real batch, optimization progress,
and checkpoint lineage; it does not promise improved policy quality or bitwise GPU
reproducibility.

## 7. Studio interaction direction

Mode: **Operate**. Preserve Studio's light, compact desktop workbench, existing
panels, and shared Timeline. Support mouse, touchpad, and keyboard at a minimum
viewport of 1024 CSS pixels. No mobile or touch-specific layouts.

| Activity | Primary job | Key transition |
| --- | --- | --- |
| Data | Browse episodes and inspect synchronized camera/joint evidence. | Open a dataset in Workflow, or inspect an episode from a completed result. |
| Workflow | Describe the selection, inspect SQL and its native plan, run the query, and review output. | **Draft → Run → Completed → Use in Training**. |
| Training | Configure a training pipeline against a pinned completed workflow output. | Validate input mappings, then start the existing training lifecycle. |

Within Workflow, keep the source, readable predicate, query state, and primary
action easy to scan. The graph explains the pipeline; an output table provides
included/excluded/unknown views with evidence links. Column details expose producer,
meaning, coverage, and version when needed. Keep technical provenance in details
rather than making raw storage identifiers the main authoring experience.
Optional clarification questions live in the Assistant alongside the draft, with
visible pending/answered states. They refine authoring without introducing another
primary activity or an extra confirmation step before **Run**.

Review lives inside Workflow and reuses Data for detailed evidence. No new primary
activity or separate selection-management application is needed. Cross-activity
actions carry explicit dataset/output, episode, and evidence-time identities while
preserving each activity's layout and local state.

Additional required states: no matches, missing top-camera mapping, unavailable
model, unavailable historical source, draft changed during execution, source update,
publication conflict, and lost connection. No-match results are valid completed
outputs, but cannot start a training run with zero usable examples. If the draft
changes during a run, completion belongs to the pinned revision it actually ran.

## 8. Proposed walkthrough and delivery sequence

A presenter-led 5–7 minute walkthrough is still a proposed format. Its sequence:

1. Open the already loaded dataset and inspect two episodes with multiple cameras
   and joint traces. Identify a visible lighting difference.
2. In Workflow, type the dim-top-camera instruction. Inspect the agent's chosen
   interpretation, camera mapping, derived-column definition, episode predicate,
   SQL, and native query plan. If clarification UI is included, demonstrate one useful
   follow-up and how its answer changes the same draft.
3. Click **Run**. Show real computation/reuse and the transition to a
   completed output with full population counts.
4. Review included, excluded, and unknown episodes. Open supporting evidence in
   Data. Adjust a threshold, click **Run**, and demonstrate selection changes using
   stored values.
5. If supported and appropriate to the archive, add the ball-in-bowl condition.
   Show which new values need computation and which existing values can be reused.
6. In Training, load the completed workflow output. Inspect a real batch and run a
   short job with checkpoint lineage back to the workflow and source.

The focal moment is a semantic request becoming SQL, an inspectable native plan,
and reusable data: the engineer can inspect and train from the condition without
paying for its model evaluation on each use. Long jobs may use clearly labeled earlier real
runs, but the walkthrough must demonstrate actual native execution and reuse, not
fabricated progress.

The [feature milestones](spiral-demo-milestones.md) break this flow into twelve core
tasks and three optional tasks. Each task delivers something the engineer can use,
including its UI, execution, persistence, and verification. The review-size target
is up to roughly 5,000 handwritten changed lines including tests; keep smaller
features small. Native Spiral fixes belong to the feature that needs them, without
a separate capability-audit milestone. The SQL cutover is implemented; later milestones remain planned.

| Milestone | Acceptance evidence |
| --- | --- |
| M1. Inspect an episode | Watch multiple cameras together and inspect joint motion at the same playhead. |
| M2. Describe the dataset to keep | Build and refine a filter with the agent, then save and switch between named workflows. |
| M3. Run and review the dataset | Execute native SQL inference, replace executable graphs with SQL and plan visualization, add native reuse, and produce/review durable population outputs. |
| M4. Train from the workflow | Configure and preview actual training examples, run ACT, and inspect the source data behind a checkpoint. |

The next implementation sequence is:

1. **3.1a — Native SQL inference (upstream):** implement model functions inside Spiral and prove one
   episode's decode → inference → SQL decision through the existing query path.
2. **3.1b — SQL workflow cutover:** author SQL, render structured native plans with Dioxus Flow,
   and route Run to the query engine. Logical/physical inspection consumes the native JSON contract.
   Delete the Studio graph executor and external inference path in the same cutover.
3. **3.2 — Native reuse:** implement compatible inference reuse and safe scanner
   pushdown in Spiral; keep ordinary native execution for other plans.
4. **3.3–3.5 — Durable population outputs and review:** add native atomic result
   publication, reconnect recovery, threshold refinement, and source updates.
5. **M4 — Training:** consume pinned completed outputs through the existing worker.

Preserve M2's document identity, conversation ownership, explicit Save/Discard, and
stale-edit protection through the cutover. Version the SQL document format. Convert
an old graph only when equivalence is proven; otherwise explain that it must be
reauthored, preserve the saved document, and do not silently reinterpret or execute
it through a retained legacy runner. Update `workflow-authoring.md` and the affected
API documentation with the implementation; they now describe the SQL contract. Completed graph-era work remains historical
evidence and does not mark the SQL cutover complete.

M3 delivers SQL execution and reusable dataset results. The requested end-to-end demo
includes M4; a saved query or bounded preview alone does not complete it. Optional
Q adds clarification questions, including returning to pending questions. Optional
B adds detection preview and a combined bowl/lighting filter. Neither is on the
primary dim-camera demonstration's critical path.

Outside the initial scope: new import formats, live recording or physical robot
motion, arbitrary graph editing, a general annotation system, interval selection,
multiple workflow mixtures, distributed training, branch-merge UI, and policy-quality
claims. The optional bowl predicate is a scoped model extension; a general model
catalog is not a prerequisite.

## 9. Existing foundations and native work within features

This is a source review, not a live integration test. The milestone breakdown
contains the implementation starting points. Findings below supersede stale
assumptions in earlier drafts; inspect the relevant code and native behavior while
developing each feature.

| Area | Existing evidence / dependency to verify |
| --- | --- |
| Native dataset | Earlier migration notes report `dataset_default`, 52 episodes, 10 topics, and 66,549 decoded messages. Recheck the actual population and camera/action contents. |
| Episode filtering | Current episode-filter notes document the upstream UUID comparison fix in Spiral PR #7033; exact Arrow UUID binding is implemented. Revalidate the installed build with two members and an empty result. |
| Media inspection | Native H.264 packet reads and single-camera WebCodecs playback/keyframe seeking are implemented. Multi-camera synchronization and joint inspection are recorded as implemented in M1. Browser decoding does not prove native model/training images. |
| Workflow | SQL authoring, native capability discovery, Dioxus Flow plan display, and native query execution are implemented. Named workflows, Save/Discard, and conversation ownership are preserved. Full episode inference verification remains upstream-dependent. |
| Models | Existing SigLIP image/concept scoring is not a verified object detector or temporal task-success assessor. Choose and validate the actual producers; do not advertise unsupported ones. |
| Persistence | Legacy feature-cache code exists but the current runner does not use it. Native source-version-aware reuse and immutable completed outputs with full membership are planned. Prove historical dependency resolution and retention. |
| Training | ACT and the worker job/checkpoint lifecycle exist. The Python MCAP dataset constructor currently raises unavailable, and Studio explicitly blocks these jobs. Implement native worker-owned resolution/loading and completed-output preflight; do not merely remove the blocker. |
| Recovery/runtime | Record the actual local driver build and a verified recovery method for the demo project; older fixed-table project bundles may not preserve native datasets. |

Spiral bugs or missing features are part of the work needed to deliver the owning
feature. Fix native support along the way, with an upstream PR when appropriate,
and verify the Studio feature against it. The native fix must land before a Studio
change that depends on it can be independently merged. Ask before choosing a
workaround; do not add alternate identity filters, a second database, a local
version catalog, or substitute media storage to bypass the intended native path.

Source starting points:

- [Current catalog layout](dataset-namespaces.md), [MCAP migration/import](mcap-import-design.md),
  [episode-filter investigation](mcap-episode-filter.md), [database boundary](database-boundary.md).
- [Native dataset storage](../crates/backend/storage/src/datasets.rs),
  [episode browser](../crates/frontend/src/activities/data/episode_browser.rs).
- [Workflow model execution](../crates/backend/src/workflow/model.rs),
  [feature persistence](../crates/backend/storage/src/workflow_features.rs).
- [Training contracts](../crates/common/src/training.rs),
  [training execution](../crates/backend/src/training/execution.rs).
- [Product intent](../PRODUCT.md), [Studio design system](../DESIGN.md).

## 10. Agreed decisions and details to resolve

| Decision | Current position |
| --- | --- |
| Selection grain | Whole episodes, as requested. Frames/intervals explain evidence; windows belong to Training. |
| Natural-language authoring | Required for this demo. Generated definitions remain inspectable and authoritative. |
| Execution boundary | Explicit **Run** submits SQL to Spiral. No separate preparation stage or Studio executor; Training consumes completed outputs. |
| Workflow authority | SQL is authoritative; separately bound parameters are future work. Native logical/physical plans are derived views, not stored executable graphs. |
| Native AI contract | Define actual catalog signatures, version/determinism semantics, batching, and execution semantics. JSON plan inspection is implemented through Dioxus Flow; cancellation is out of scope. |
| Lighting semantics | The agent chooses camera mapping, supported producer, aggregation, coverage, and thresholds for each request, using available evidence and exposing its interpretation. |
| Bowl semantics | Optional second predicate. The agent decides temporal meaning from context and may ask a focused follow-up; final-state persistence is an example, not a fixed default. |
| Clarification UI | Optional enhancement: agent-initiated questions with suggested answers, free text, and explicit delegation when a reasonable default exists. |
| Unknown policy | Agent-owned, visible, and versioned. Suggested starting point: exclusion with a separate review population; preserve unknown status regardless of membership. |
| Augmented outputs | Persistent derived fields available with the source; optional downstream mappings must match the consumer's contract. |
| Revision UX | Proposed automatic completed-output revision on a successful Run, including predicate-only reevaluation, without a separate Publish step. |
| Native implementation | Develop and verify required Spiral support within each feature, including upstream fixes as needed. |

## Iteration log

- **2026-09-09 · Draft 1:** reviewed the homepage, Studio contracts, and migration
  notes; proposed an interval/failure investigation and published-selection demo.
- **2026-09-09 · Draft 2:** revised around the requested dataset of MCAP episodes,
  multi-camera/joint inspection, natural-language episode selection, explicit
  preparation of persistent ML columns, and Training consuming a prepared workflow.
  Whole-episode membership replaces interval curation for this demo. Preparation
  now owns the reusable output; separate selection publication is removed from
  the primary flow. No implementation or live driver/model/training validation
  performed in this iteration.
- **2026-09-09 · Draft 3:** assigned interpretation of vague instructions to the
  workflow agent, replacing human-owned semantic decisions with inspectable,
  context-dependent agent choices. Added optional follow-up-question UX that
  refines the same draft while preserving explicit dataset preparation. Design
  document changes only.
- **2026-09-09 · Draft 4:** studied the current implementation at `e2c818a`, corrected
  the filtering/playback/training baseline, and added the linked milestone breakdown
  with PR-sized scopes, dependencies, merge results, and verification lanes. No
  application code changed and no runtime tests were executed in this planning pass.
- **2026-09-09 · Draft 5:** replaced technical-layer tasks and preliminary capability
  gates with ten core user-facing feature tasks and three optional tasks. Increased
  the review-size target to roughly 5,000 handwritten changed lines, retaining Done
  checkboxes. Required Spiral fixes now belong to the feature being developed.
  Documentation changes only.
- **2026-09-11 · Draft 6:** agreed that workflows are SQL plus parameters and their
  graphs visualize Spiral query plans. Replaced the separate preparation flow with
  Run; moved inference, optimization, reuse, and materialization into Spiral. Split
  the next delivery into native SQL inference and Studio cutover, followed by reuse
  and durable outputs. Recorded removal of the Arrow graph runner and legacy
  document handling. Documentation changes only; no runtime verification performed.
