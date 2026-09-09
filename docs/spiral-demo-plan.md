# Spiral robotics demo plan

Status: **draft for iteration**, September 9, 2026. This iteration changes the
design document only; it does not authorize implementation. Product requirements
from the discussion are distinguished below from proposed semantics and unresolved
Spiral capabilities. Earlier repository findings remain leads to verify, not proof
that the native data path works today.

Implementation is organized in [the milestones](spiral-demo-milestones.md).
Each task has a Done checkbox for tracking completion.

## 1. Goal and agreed direction

An ML engineer has a dataset already loaded into Spiral. The dataset contains
multiple episodes; each episode originated as one MCAP file containing multiple
camera feeds and joint samples. Studio should let the engineer inspect those
recordings, describe which episodes to keep, prepare the derived data that the
selection needs, and use the workflow as an input to training.

The central demonstration is:

> Inspect the dataset. Ask to exclude episodes whose top camera is too dim.
> Prepare the dataset to compute the required columns. Review the selected
> episodes, then train from that workflow without recomputing its ML predicates.

An optional second condition is “also exclude episodes where the ball was not
placed in the bowl.” This exercises a pipeline with object detection or another
appropriate visual model, followed by an episode-level decision.

The following are requirements from the discussion:

- **Data** exposes episodes, their multiple camera feeds, and joint samples for
  synchronized inspection. Import has already happened before this flow starts.
- **Workflow** starts with human-written instructions and generates an inspectable
  pipeline that selects whole episodes and may add derived outputs to the dataset.
- **The workflow agent** interprets vague instructions and chooses concrete,
  executable semantics. Optional follow-up questions can help refine the workflow;
  users are not required to specify every parameter or resolve every ambiguity.
- **Prepare dataset** is an explicit user action. Before preparation, required ML
  columns may be absent; afterward, the computed columns are persisted and reusable.
- **Training** can choose a workflow as its data source. Its input path consumes
  prepared outputs and never evaluates the workflow's enrichment models on the fly.

Proposed product headline: **Describe the episodes you want, prepare them once,
and use the same workflow to inspect and train.**

The [Spiral homepage](https://spiraldb.com/) was a presentation reference in the
first draft. The episode → workflow → preparation → training flow above now owns
the design. Website examples are not evidence of installed driver capabilities.

## 2. Product model

There are three user-facing objects, with revisions underneath them:

| Object | Meaning |
| --- | --- |
| Dataset | The source collection of MCAP episodes already in Spiral, with camera and joint streams. Source recordings remain available after filtering. |
| Workflow | A saved, editable recipe: source dataset, derived-column definitions, episode predicates, and output fields. Natural language produces this recipe; the saved executable definition is authoritative. |
| Prepared workflow output | The result of applying a workflow revision to a specific source snapshot: persisted derived values, coverage, and resolved episode membership. Inspection and Training consume this same result. |

“Remove episodes” means exclude them from the workflow output. It does not delete
source MCAP files or source episodes. “Augment the dataset” means expose persistent
derived columns alongside the original data through native Spiral relations;
physical persistence must use verified Spiral capabilities. It does not require
rewriting the MCAP files or copying every recording into a new dataset.

Preparation produces a reusable data product, not just an execution cache. A
prepared output records the source and workflow revisions, producer definitions,
coverage, and selected episode identities. Model-derived outputs may serve only
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
prepared output reference, episode identity, and relevant evidence time. It can
show a derived value or detection overlay when that output exists, while keeping
the original recordings available for comparison. Evidence timestamps are not
selection boundaries: an excluded episode is excluded as a whole.

Start by auditing the current archive for actual camera roles, joint/action
channels, and visible task variation. The dim-camera example is the primary story;
the bowl example depends on suitable recordings and a validated producer.
An archive with only joint states can still be inspected, but action-target
availability must be resolved before promising policy training.

## 4. Workflow view: instructions become a pipeline

The engineer selects an existing Spiral dataset and types:

> Select all episodes, but remove the ones whose top camera is too dim.

The Assistant's LLM agent interprets the request and generates a saved draft with
a readable explanation and an inspectable executable pipeline. Natural language is the primary authoring path
for this demo. SQL, model parameters, and the graph remain available for inspection
and refinement; the user should not need to write SQL to complete the walkthrough.

The draft identifies:

- The source dataset and population to process.
- Which camera topic represents the top camera.
- Required derived columns, their producers, and their output grain.
- How frame-level observations become episode-level values.
- The episode predicate and fields exposed by the output.

A conceptual pipeline for the two-condition example is:

```text
Source dataset snapshot: all episodes
    ├─ Top-camera frames → lighting measure/model → episode lighting summary
    └─ Task-camera frames → visual model → episode ball-in-bowl assessment
                            ↓
             Persist derived columns and coverage
                            ↓
           Evaluate whole-episode inclusion predicate
                            ↓
              Prepared workflow output
                  ├─ Inspect in Data
                  └─ Use in Training
```

This is a logical plan, not a requirement for arbitrary graph editing or a new
execution engine. Reuse Studio's relation-first workflow mechanics and supported
native operations. Producers can be deterministic calculations or ML models;
choose them for the requested meaning. Image brightness may suit a measured rule,
while semantic task outcomes may need visual models and temporal aggregation.
Object detection alone does not establish that a ball was placed in a bowl.

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
- If the answer changes an already prepared workflow, use the existing revision
  and invalidation rules: reuse compatible columns and identify any new preparation
  required. Asking or answering a question does not launch preparation.

Without this enhancement, the agent still makes reasonable semantic decisions
and explains them; users can refine the draft through ordinary instructions.
Genuinely missing required inputs remain explicit blockers. Follow-up UI is
optional scope, while agent ownership of interpretation is part of the core flow.

## 5. The two stages: unprepared and prepared

### Stage 1 — Define an unprepared workflow

The workflow definition exists, but required derived columns are missing or
incompatible with its current source/producer definitions. The engineer can inspect
source data and review the plan. The primary action is **Prepare dataset**.

Before preparation, show what will run: source episode count, camera inputs,
required outputs, model or calculation definitions, and compatible outputs already
available for reuse. Final selected-episode counts remain unavailable until the
required population is processed. A bounded sample preview, if offered, must be
an explicit operation and show its actual coverage; it cannot stand in for full
preparation or silently authorize it.

Clicking **Prepare dataset**:

1. Pins the source snapshot and workflow revision for this run.
2. Resolves required inputs and validates producer availability and semantics.
3. Computes missing compatible frame-level outputs and episode summaries, with
   measured progress and explicit errors or unknown assessments.
4. Persists derived values with source/producer provenance and coverage.
5. Evaluates the episode predicate and records the complete selected membership,
   exclusion reasons, and unknown population, independently of preview limits.
6. Makes the completed prepared output available to inspection and Training.

Preparing all required columns and freezing the resulting membership are part of
one user action. A separate “Publish selection” step is not required for this demo.
Implement and verify the required native commit/materialization behavior as part
of the preparation feature, fixing Spiral where needed. No partial output can
appear as completed.

### Stage 2 — Use a prepared workflow

The workflow reads persisted derived columns to evaluate its selection. Opening
results, inspecting episodes, or starting training does not invoke enrichment
models. Show included, excluded, and unknown counts against the source episode
count, with reasons and evidence for each decision. One episode can fail several
predicates, so reason counts may overlap even though the three outcome populations
do not.

The same prepared output supports both browsing and training. Training pins its
reference and exact membership rather than resolving a moving “latest” query on
every batch. Original camera decoding, alignment, normalization, and training-time
augmentation remain ordinary data loading operations. Policy optimization still
runs its own model; the prohibition concerns workflow enrichment inference in the
training input path.

### Operational states

The two product stages have the following operational states:

| State | Visible meaning and action | Training behavior |
| --- | --- | --- |
| Needs preparation | Required outputs or membership are not available for this draft. Show **Prepare dataset**. | Cannot use this draft. |
| Preparing | Show actual processed episodes/frames, reused work, remaining work when known, and **Cancel**. | Cannot use the in-progress output. |
| Prepared | Complete output and coverage are available. Show **Inspect results** and **Use in Training**. | May use this output after training compatibility checks. |
| Preparation failed or canceled | Show the failed stage, affected inputs, and retry action. Any partial work remains incomplete. | Cannot use the incomplete output. |
| Needs update | The draft or selected source differs from the last prepared output. Explain what must be recomputed. | Must explicitly choose an earlier prepared revision or prepare the current draft. |

A successful preparation can contain computed unknowns under its declared policy;
unexpected execution/decode failures must not silently become ordinary unknowns.
Retry may reuse valid completed work only when the native persistence contract
supports it. A last successful output remains identifiable and usable as that
older revision; never label it as the current draft's result.

### Reuse and changes

| Change | Required work |
| --- | --- |
| Repeat the same workflow against the same source and producers | Reuse compatible prepared outputs. |
| Adjust a threshold over a stored score/summary | Re-evaluate selection and create a new output revision; no model inference. |
| Change an aggregation rule with sufficient stored observations | Recompute the summary and selection; reuse underlying observations. |
| Change model, concept prompt, preprocessing, camera, or sampling semantics | Prepare affected derived columns again unless an exactly compatible result exists. |
| Add another model-derived condition or output | Prepare its missing dependencies; reuse existing compatible columns. |
| Add or modify source episodes | Explicitly prepare a new source snapshot; compute affected/missing values where valid reuse is possible. |

Reuse identity includes source version and producer semantics, not just a column
name. Adding a new predicate does not erase an earlier prepared result or alter
an existing training run. Compatible derived columns should be reusable across
workflows over the same source, subject to verified native storage/version support.

## 6. Training view: choose a workflow as input

In Training, the engineer chooses **Workflow** as the data source, then a saved
workflow and prepared output revision. **Use in Training** from Workflow opens the
same configuration with those references already filled in.

Before **Start training**, show:

- Workflow name/revision, prepared output reference, and source snapshot.
- Selected episode count and separately the count of valid training windows.
- Camera, joint-state, and action mappings required by the selected training pipeline.
- Optional derived output mappings, only when supported by that pipeline.
- Sampling/context/action horizon, split policy, and normalization configuration.

A prepared workflow is not automatically training-compatible. Missing action
channels, incompatible shapes, insufficient valid windows, or unavailable runtime
must have specific explanations. If preparation is required, link back to that
workflow and its **Prepare dataset** action. Starting training must not trigger
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

A training run pins the prepared output, loader configuration, and resulting input
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
| Data | Browse episodes and inspect synchronized camera/joint evidence. | Open a dataset in Workflow, or inspect an episode from a prepared result. |
| Workflow | Describe the selection, inspect the generated plan, prepare derived columns, and review output. | **Needs preparation → Prepare dataset → Prepared → Use in Training**. |
| Training | Configure a training pipeline against a pinned prepared workflow output. | Validate input mappings, then start the existing training lifecycle. |

Within Workflow, keep the source, readable predicate, preparation state, and primary
action easy to scan. The graph explains the pipeline; an output table provides
included/excluded/unknown views with evidence links. Column details expose producer,
meaning, coverage, and version when needed. Keep technical provenance in details
rather than making raw storage identifiers the main authoring experience.
Optional clarification questions live in the Assistant alongside the draft, with
visible pending/answered states. They refine authoring without introducing another
primary activity or an extra confirmation step before **Prepare dataset**.

Review lives inside Workflow and reuses Data for detailed evidence. No new primary
activity or separate selection-management application is needed. Cross-activity
actions carry explicit dataset/output, episode, and evidence-time identities while
preserving each activity's layout and local state.

Additional required states: no matches, missing top-camera mapping, unavailable
model, unavailable historical source, stale draft during preparation, source update,
publication conflict, and lost connection. No-match results are valid prepared
outputs, but cannot start a training run with zero usable examples. If the draft
changes during a run, completion belongs to the pinned revision it actually ran.

## 8. Proposed walkthrough and delivery sequence

A presenter-led 5–7 minute walkthrough is still a proposed format. Its sequence:

1. Open the already loaded dataset and inspect two episodes with multiple cameras
   and joint traces. Identify a visible lighting difference.
2. In Workflow, type the dim-top-camera instruction. Inspect the agent's chosen
   interpretation, camera mapping, derived-column definition, episode predicate,
   and unprepared state. If clarification UI is included, demonstrate one useful
   follow-up and how its answer changes the same draft.
3. Click **Prepare dataset**. Show real computation/reuse and the transition to a
   prepared output with full population counts.
4. Review included, excluded, and unknown episodes. Open supporting evidence in
   Data. Adjust a threshold and demonstrate selection changes using stored values.
5. If supported and appropriate to the archive, add the ball-in-bowl condition.
   Show which new columns need preparation and which existing values can be reused.
6. In Training, load the prepared workflow output. Inspect a real batch and run a
   short job with checkpoint lineage back to the workflow and source.

The focal moment is the change from an unprepared semantic request to reusable
data: the engineer can inspect and train from the condition without paying for
its model evaluation on each use. Long jobs may use clearly labeled earlier real
runs, but the walkthrough must demonstrate actual preparation and reuse, not
fabricated progress.

The [feature milestones](spiral-demo-milestones.md) break this flow into ten core
tasks and three optional tasks. Each task delivers something the engineer can use,
including its UI, execution, persistence, and verification. The review-size target
is up to roughly 5,000 handwritten changed lines including tests; keep smaller
features small. Native Spiral fixes belong to the feature that needs them, without
a separate capability-audit milestone. These are planned features; this document
does not start implementation.

| Milestone | Acceptance evidence |
| --- | --- |
| M1. Inspect an episode | Watch multiple cameras together and inspect joint motion at the same playhead. |
| M2. Describe the dataset to keep | Build and refine a filter with the agent, then save and switch between named workflows. |
| M3. Prepare and review the dataset | Try a filter on one episode, prepare the population, review/refine selection, retry preparation, and process added episodes. |
| M4. Train from the workflow | Configure and preview actual training examples, run ACT, and inspect the source data behind a checkpoint. |

M3 is a useful inspection/preparation milestone. The requested end-to-end demo
includes M4; a saved query or prepared preview alone does not complete it. Optional
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
| Media inspection | Native H.264 packet reads and single-camera WebCodecs playback/keyframe seeking are implemented. Multi-camera synchronization and imported joint traces remain work. Browser decoding does not prove native model/training images. |
| Workflow | Relation-first Query/Model/Result, revision persistence, Assistant conversations, and SigLIP execution exist. The active graph defaults to one identity; named workflow selection, native inputs, and explicit preparation admission require changes. |
| Models | Existing SigLIP image/concept scoring is not a verified object detector or temporal task-success assessor. Choose and validate the actual producers; do not advertise unsupported ones. |
| Persistence | Mutable feature-cache/registry transactional publication exists, with source-relation-based reuse. Source-version-aware identities and immutable prepared outputs with full membership are new. Prove historical dependency resolution and retention. |
| Training | ACT and the worker job/checkpoint lifecycle exist. The Python MCAP dataset constructor currently raises unavailable, and Studio explicitly blocks these jobs. Implement native worker-owned resolution/loading and prepared-input preflight; do not merely remove the blocker. |
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

## 10. Decisions remaining

| Decision | Current position |
| --- | --- |
| Selection grain | Whole episodes, as requested. Frames/intervals explain evidence; windows belong to Training. |
| Natural-language authoring | Required for this demo. Generated definitions remain inspectable and authoritative. |
| Preparation boundary | Explicit **Prepare dataset** computes/persists derived columns and completes the workflow output. Training never performs enrichment. |
| Lighting semantics | The agent chooses camera mapping, supported producer, aggregation, coverage, and thresholds for each request, using available evidence and exposing its interpretation. |
| Bowl semantics | Optional second predicate. The agent decides temporal meaning from context and may ask a focused follow-up; final-state persistence is an example, not a fixed default. |
| Clarification UI | Optional enhancement: agent-initiated questions with suggested answers, free text, and explicit delegation when a reasonable default exists. |
| Unknown policy | Agent-owned, visible, and versioned. Suggested starting point: exclusion with a separate review population; preserve unknown status regardless of membership. |
| Augmented outputs | Persistent derived fields available with the source; optional downstream mappings must match the consumer's contract. |
| Revision UX | Proposed automatic prepared-output revision on successful preparation or predicate-only re-evaluation, without a separate Publish step. |
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
