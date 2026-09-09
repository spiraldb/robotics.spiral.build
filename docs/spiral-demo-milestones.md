# Spiral demo feature milestones

Status: **design for review**, September 9, 2026. This document plans the features
in [spiral-demo-plan.md](spiral-demo-plan.md); it does not start implementation.
It uses the earlier source review at `e2c818a`. No new runtime validation was
performed for this revision.

**Progress:** choose an unchecked task whose feature dependencies are complete.
Mark its Done checkbox when its user-visible behavior works and verification passes.
This breakdown replaces the previous task IDs; none were marked complete.

## How to use this plan

Each task answers **“What can the engineer do after this lands?”** It includes
whatever UI, backend, persistence, and Spiral changes that behavior needs. A new
protocol type, schema, or capability probe is implementation work within a feature,
not a milestone of its own.

Use **roughly 5,000 handwritten changed lines per review** as the upper sizing
target, including tests. Keep smaller features small; there is no reason to fill
the budget. If a feature grows beyond that, split it into smaller usable behaviors
and update the task list. Keep its UI, backend, and persistence together when they
fit. Report generated changes and upstream changes separately; apply the same
review-size target to an upstream PR rather than hiding its size.

Each task should merge on top of its completed dependencies and be demonstrable
without an unfinished sibling. Include the necessary compatibility handling and
focused tests in that task. Reuse existing owners and modules; avoid unrelated
refactors. The first working version can have a narrow scope, such as evaluating
one episode or previewing one training example, as long as that scope is useful
and explicit in the product.

**Spiral work happens with the feature that needs it.** If native image decoding,
historical reads, or another operation is missing, implement/fix it in Spiral as
part of delivering that feature, with an upstream PR when appropriate. Verify the
feature against that fix and land the required native support before its Studio
consumer. There is no preliminary capability-audit milestone. A missing capability
is work to do, not a reason to redefine the feature. Keep the repository's rule
against working around Spiral limitations; ask before choosing a workaround.

## Existing features to build on

The earlier source review found these foundations. Recheck the relevant code when
starting a feature; this is not a new live integration result.

| Feature today | Reuse | What the next feature adds |
| --- | --- | --- |
| Browse datasets, episodes, and topics | [Dataset storage](../crates/backend/storage/src/datasets.rs), [episode browser](../crates/frontend/src/activities/data/episode_browser.rs), [topic inspector](../crates/frontend/src/activities/data/episode_topics.rs) | An episode workspace with cameras and joints together. Import is already outside this flow. |
| Play one recorded camera | [Native packet reads](../crates/backend/storage/src/datasets/playback.rs), [WebCodecs decoder](../crates/frontend/src/activities/data/episode_decoder.rs), [player](../crates/frontend/src/activities/data/episode_playback.rs) | Multiple feeds sharing one clock. Exact UUID reads and keyframe seeking already exist. |
| Render recorded joint traces | [Episode cache](../crates/frontend/src/activities/data/episode_cache.rs), [Timeline](../crates/frontend/src/panels/timeline/panel.rs) | Read imported native member/topic samples, without requiring Studio-private recording headers or a filename lookup. |
| Ask the Workflow Assistant to build a graph | [Agent](../crates/backend/src/workflow/agent.rs), [manager](../crates/backend/src/workflow/manager.rs), [Workflow UI](../crates/frontend/src/activities/workflow/workspace.rs) | Episode-filter semantics, explicit preparation, named workflows, and optional questions. The existing agent can currently launch Model work. |
| Run SigLIP and retain feature cells | [Model runner](../crates/backend/src/workflow/model.rs), [feature persistence](../crates/backend/storage/src/workflow_features.rs) | Native MCAP inputs, source-version-aware reuse, episode decisions, and prepared workflow outputs. Existing reuse uses source-relation identity. |
| Save a graph and its revisions | [Workflow documents](../crates/backend/storage/src/workflow_documents.rs), [common contracts](../crates/common/src/workflow.rs) | Multiple named workflows and durable complete output membership. Current query previews are bounded results, not complete training selections. |
| Configure ACT and manage jobs/checkpoints | [Training manager](../crates/backend/src/training/manager.rs), [worker jobs](../training/src/spiral_training_runtime/jobs.py), [training UI](../crates/frontend/src/activities/training/workspace.rs) | Prepared-workflow inputs and native training examples. The [MCAP loader](../training/src/spiral_training_runtime/mcap.py) currently raises unavailable. |

The worker continues to own training reads, batches, optimization, and checkpoint
writes. Existing Arrow batch transport serves rollout inference; these features
do not require introducing a Studio-to-worker training-batch upload architecture.

## Milestones at a glance

| Milestone | What the engineer can do | Tasks |
| --- | --- | --- |
| M1 — Inspect an episode | Watch its cameras together and inspect the joints at the same time. | 1.1–1.2 |
| M2 — Describe the dataset to keep | Ask for an episode filter, save it, and refine its meaning. | 2.1–2.2 |
| M3 — Prepare and review the dataset | Try a filter, prepare the population, inspect decisions, and reuse the work. | 3.1–3.4 |
| M4 — Train from the workflow | Choose prepared data, inspect training examples, configure sampling, and train. | 4.1–4.2 |
| Optional Q — Clarify an instruction | Answer an agent question and return to an unanswered one later. | Q.1 |
| Optional B — Filter by ball-in-bowl outcome | Inspect visual detections and use the outcome alongside lighting. | B.1–B.2 |

There are **10 core feature tasks and three optional tasks**. M1 and M2 can start
independently. The main sequence is M2 → M3 → M4, with M1 supplying the inspector
used by result review and training-example preview. Q and B can be added when
wanted without holding up the initial lighting-filter flow.

## M1 — Inspect an episode

### 1.1 — Watch all cameras on one timeline

- [ ] Done

**After this lands:** opening an episode lets the engineer view its recorded
cameras together, play/pause them, and scrub one shared timeline.

**Build:** extend the existing episode/topic workspace and single-camera decoder.
Move transport ownership to the episode and open its camera panels through the
existing workbench. Preserve topic-table inspection. Load only the selected native
member; show each camera's gaps or errors and bound combined decoder memory.

**Verify:** open two episodes in turn; scrub cameras with different start times and
frame rates across keyframes. No camera shows a different episode or runs on its
own clock. Exercise missing feeds, panel movement/closure, and keyboard transport
at 1024 pixels. Extend the existing native playback and browser seek tests.

**Depends on:** none. Reuse today's native playback as the starting point.

### 1.2 — Inspect joint motion beside the cameras

- [ ] Done

**After this lands:** the engineer sees joint traces and current sample values at
the camera playhead, including recorded actions when available.

**Build:** read the selected native member's actual joint/action topics and connect
them to the existing trace renderer. Preserve timestamps, units, nulls, and source
field names. Start with the real demo dataset's schema; show unsupported channels
explicitly. Do not derive historical values from live hardware or calibration.

**Verify:** compare displayed values/times with source samples while scrubbing;
switch episodes; inspect a gap and an episode without action data. A bounded topic
preview must not be presented as a complete trajectory. Camera and joint evidence
remain aligned without a physical robot connection.

**Depends on:** 1.1.

## M2 — Describe the dataset to keep

### 2.1 — Build and refine an episode filter with the agent

- [ ] Done

**After this lands:** the engineer chooses a dataset and types “select all episodes
but remove the ones whose top camera is too dim.” The agent creates an inspectable
workflow draft and explains its chosen interpretation. The engineer can correct
that meaning in plain language and see the same draft update.

**Build:** extend the existing Assistant's discovery and graph authoring for native
episodes. The agent chooses the actual camera, producer, sampling, episode rule,
and unknown policy. Show the resulting explanation, required columns, and
**Needs preparation** state. Drafting must not run enrichment; enforce that in the
backend as well as the agent tools. Read-only inspection remains available. Apply
corrections such as “use the wrist camera” or “only when most of the episode is
dark” to the executable definition and explanation together, preserving identity
and revision history.

**Verify:** the real instruction produces a valid saved draft against the actual
schema without asking the human to set every parameter. An absent camera is
explained. A simulated provider calling the old run tool cannot trigger Model work
while authoring. Apply two corrections, reopen the draft, and confirm its graph
and explanation agree. No model is advertised as implemented just because the
agent named it.

**Depends on:** none. This is a usable authoring feature before execution is added.

### 2.2 — Save and switch between named workflows

- [ ] Done

**After this lands:** the engineer can name the lighting workflow, create another
workflow, and return to either one with its own definition and conversation context.

**Build:** add create/name/select behavior to the current workflow persistence and
picker. Carry real workflow identities through the existing manager, Assistant,
and UI instead of hard-coding the default graph. Preserve existing history. Keep
current serialized execution; concurrent workflow scheduling is unnecessary.

**Verify:** create two workflows, edit them independently, reconnect, and switch
between them. Delayed responses and Assistant saves cannot change the wrong graph.
The existing default workflow remains readable after the persistence change.

**Depends on:** 2.1.

## M3 — Prepare and review the dataset

### 3.1 — Try the filter on one episode

- [ ] Done

**After this lands:** the engineer explicitly tries the draft on a chosen episode
and sees its model-derived values, episode decision, and supporting frame times.

**Build:** connect the real native camera input to the existing model runner and
persist the sampled observations and episode summary with source/producer identity.
Use the producer selected by the workflow; pinned SigLIP is the existing starting
point when its score matches the interpretation. Implement missing native decode
support in Spiral here. Label the result as a one-episode preview, not a prepared
population. This preview is an explicit action, not a side effect of authoring.

**Verify:** run on a bright and a dark episode, inspect actual model outputs, and
compare evidence times to the recording. Repeating the same preview reuses compatible
values; changing source bytes or producer semantics does not. Missing evidence is
unknown, not a successful assessment. Confirm native image values, not just SQL shape.

**Depends on:** 2.1.

### 3.2 — Prepare the full dataset

- [ ] Done

**After this lands:** clicking **Prepare dataset** evaluates all required episodes
and produces a saved, prepared workflow output ready for inspection and downstream
use. The engineer can cancel, leave the activity, and retry later.

**Build:** expand the working one-episode path to the source population. Show real
progress; persist required columns, coverage, inclusion decisions, and complete
membership against pinned source/workflow revisions. Only a completed output becomes
Prepared. Include native snapshot/atomic publication support in this feature where
needed. Reuse existing backfill and execution owners instead of adding another
runner. Include cancel/retry controls and reconnect recovery. Retain the last
completed output and reuse successful compatible dependency work; do not claim
resumability for partial values that were never durably committed.

**Verify:** the included/excluded/unknown populations reconcile with the source.
Test a population larger than the preview limit, reconnect, and reopen the result.
A failed or canceled run cannot appear Prepared; exceeding a supported bound cannot
silently truncate membership. Cancel during computation and near publication,
reconnect, and retry; actual reused/computed counts must match durable work.
Source advance cannot change a completed output.

**Depends on:** 3.1. Durable output identity is part of this feature, not deferred
until Training needs it.

### 3.3 — Review and refine which episodes are selected

- [ ] Done

**After this lands:** the engineer sees why episodes were included, excluded, or
left unknown, opens their evidence, and adjusts the rule without rerunning models
when the required values are already prepared.

**Build:** add decision views with derived values, reasons, accurate counts, and
paginated results. Connect **Inspect episode** to the existing multi-camera/joint
inspector with pinned source/member/evidence-time identity. Apply natural-language
or manual edits through dependency-aware re-evaluation and save a new output
revision. Explain when a changed camera/model/sampling rule needs preparation;
keep the previous output selectable.

**Verify:** inspect one episode from each outcome and change a lighting threshold.
The selection changes with zero enrichment calls; a producer change requests new
preparation. Reopen both outputs after source advance and confirm independent
membership and original evidence. Overlapping reasons do not duplicate episodes,
and filtering never deletes source recordings. Reuse observations for a changed
aggregation only when their stored coverage supports it.

**Depends on:** 3.2, 1.2.

### 3.4 — Prepare newly added episodes without losing the old result

- [ ] Done

**After this lands:** after the source dataset grows, the engineer sees that the
workflow has new data to prepare and can produce an updated output.

**Build:** show source changes relative to the prepared revision and prepare the
new snapshot. Reuse observations for provably unchanged native members with the
same producer semantics; compute new/changed inputs. Preserve earlier outputs
and their source dependencies so ongoing training can keep using them.

**Verify:** append an episode, replace one source member, and prepare again. Counts
reflect both changes; unchanged inputs reuse safely, changed bytes do not. Opening
the old output still resolves its original membership and recording evidence.

**Depends on:** 3.3.

## M4 — Train from the workflow

### 4.1 — Configure and preview training data from a workflow

- [ ] Done

**After this lands:** the engineer chooses a prepared workflow in Training, configures
its camera/state/action inputs, sampling and evaluation split, and inspects real
aligned training examples before starting a run.

**Build:** add a workflow/output picker and **Use in Training** navigation. Persist
that selection and show the source revision and episode population. Extend the
worker with pinned-source resolution, native image/state/action reads, valid-window
selection, and a read-only preflight/example preview shown in Studio. Implement
missing native worker image support in Spiral as part of this feature. Split by
whole episode, reject gaps/invalid horizons, and derive normalization from training
samples only. Include wire contracts and their consumers in the feature.

Show available mappings, eligible example counts, and specific compatibility errors.
An unprepared draft links back to preparation; no-match or missing-action input
cannot train. Keep Start unavailable until 4.2, while making configuration and
real example inspection fully usable. The worker owns data loading; there is no
Studio-to-worker training-batch upload path.

**Verify:** choose two workflows and an older output, configure the inputs, and
compare actual preview pixels/state/actions to known source values. Check membership,
alignment, episode boundaries, gaps, disjoint splits, deterministic sampling, and
training-only statistics. Reconnect and retain the configuration. Input resolution
and preview perform no workflow enrichment. Exercise real native reads as well as
Rust/Python contract round trips.

**Depends on:** 3.2, 2.2, 1.2.

### 4.2 — Train ACT and inspect the data behind its checkpoint

- [ ] Done

**After this lands:** the engineer starts ACT from the prepared selection, sees real
optimization progress, obtains a persisted checkpoint, and can open the exact
workflow output and episodes that supplied its training data.

**Build:** feed the validated loader into the existing worker job loop and ACT
session. Enable Start from actual readiness, retain stop/reconnect behavior, and
publish checkpoints through native storage. Record the prepared output and resolved
input/split/sampler/normalization configuration with the run/checkpoint. Expose this
lineage in existing history and checkpoint details, with links back to the prepared
result and recorded evidence. Keep the existing ACT artifact readers compatible.

**Verify:** execute a real short optimization job and reload its persisted checkpoint.
Inspect actual batch identities against the output and split, with zero workflow
enrichment calls during training. Edit the workflow and advance the dataset after
Start; neither changes the run's inputs or its checkpoint's evidence links. A failed
checkpoint write cannot appear successful. Exercise stop and reconnect, then run
the complete inspect → instruct → prepare → review → train sequence on real data.
This proves traceable training inputs, not policy improvement or bitwise GPU reproducibility.

**Depends on:** 4.1, 3.3.

## Optional Q — Clarify an instruction

### Q.1 — Answer agent questions while building a workflow

- [ ] Done

**After this lands:** the agent can ask a focused follow-up, use the answer to refine
the same workflow, and let the engineer return to an unanswered question later.

**Build:** add suggested answers, free text, and **Use your judgment** where useful
inside the existing Assistant. Bind questions/answers to workflow and conversation
identity; update the executable definition and its explanation together. Persist
pending/answered state, release active provider/lease resources while waiting, and
resume from saved context. Explain when an intervening edit makes a question obsolete.
Questions remain optional agent behavior, not a mandatory authoring form.

**Verify:** answer using each input method, reconnect with a pending question, and
switch workflows before answering. A preselected option is not a submitted answer;
a stale answer cannot change the wrong draft. Answering never launches preparation;
a change to a prepared workflow uses normal reuse/invalidation behavior. The agent
can still decide and explain semantics without asking a question.

**Depends on:** 2.2.

## Optional B — Filter by ball-in-bowl outcome

### B.1 — Inspect ball and bowl detections in an episode

- [ ] Done

**After this lands:** the engineer explicitly runs a visual producer on one episode
and inspects its ball/bowl detections beside the recorded frames.

**Build:** add one suitable pinned producer to the existing model execution and
one-episode preview. Persist its typed outputs and show the relevant evidence,
including uncertainty/occlusion. Choose it against actual recordings and implement
any needed native support with this feature. This is not a general model catalog.

**Verify:** inspect real detections on representative frames, including missing or
occluded objects, and confirm repeat evaluation reuses compatible outputs. SigLIP
scores must not be mislabeled as object detections. Detections alone do not establish
a successful placement; that episode interpretation is the next feature.

**Depends on:** 3.1, 1.1. This optional example requires recordings of the bowl task.

### B.2 — Keep episodes where the ball was placed in the bowl

- [ ] Done

**After this lands:** the engineer adds the bowl instruction to the lighting workflow,
prepares the added outputs, and reviews the resulting combined selection.

**Build:** let the agent choose and explain the temporal interpretation, aggregate
stored observations into an episode assessment, and compose it with lighting.
Use existing preparation/review/training paths; reuse compatible lighting columns.
Optional Q can clarify the interpretation, but is not required.

**Verify:** review successful, unsuccessful, and ambiguous episodes. Change “ever
inside” to “inside at the end” and inspect the difference. Confirm that adding the
condition computes only missing compatible dependencies and that Training uses the
same combined membership.

**Depends on:** B.1, 3.3.

## Verification and delivery

Each task includes its own real feature check and targeted regressions. Use the
existing [native playback tests](episode-playback.md),
[database tests](database-boundary.md), Workflow model/manager tests, browser tests,
and [Python worker tests](../training/tests/test_runtime.py) for the boundaries it
changes. Run relevant Rust/WASM/Python checks; the current default CI performs
Python source checks but does not execute the full worker test suite.

Validate on a small, representative native dataset as part of the first feature
that needs it. Use explicit disposable test resources, preserve real camera/joint
semantics, and keep test/version evidence with the feature PR. A fake boundary can
test an error path; it cannot prove that the new feature works on native recordings.

M1 is complete when cameras and joints can be inspected together. M2 is complete
when the engineer can create, save, and correct a workflow through the Assistant.
M3 is complete when the engineer can prepare, inspect, refine, retry, and update
its dataset. M4 completes the requested training flow. Optional Q/B features have
their own checkboxes and do not delay the initial lighting example.

Keep final rehearsal lightweight: run the actual product sequence after M4, including
one reconnect and one changed workflow, and record observed results. There is no
separate infrastructure or rehearsal milestone to finish before useful features land.
