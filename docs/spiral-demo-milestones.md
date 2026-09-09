# Spiral demo implementation milestones

Status: **implementation plan for review**, September 9, 2026. No implementation
is authorized by this document. It breaks down the product flow in
[spiral-demo-plan.md](spiral-demo-plan.md) using a source review of commit
`e2c818a` and the current design-document edits. No live Spiral, browser, model,
or training tests were run for this planning pass.

## 1. How to size and merge the work

Each numbered task is one intended PR, independently reviewable and verifiable
**after its listed dependencies have merged**. Independent verification does not
mean each PR delivers the entire demo. A storage operation, usable API, or narrower
user-visible behavior is a valid intermediate delivery.

- Target **700–1,100 changed lines**, counting additions plus deletions in handwritten
  source and tests. Capability probes and documentation can be smaller. Do not add
  code to reach a quota. Generated schemas, lockfiles, and binary fixtures are
  reported separately, not used to hide a large handwritten change.
- Treat **1,200 handwritten changed lines** as a re-slicing trigger. Split a task
  before implementing beyond that size; these ranges are estimates, not guarantees.
  Unknown upstream work has no LOC commitment until the native interface is known.
- Every PR includes its own relevant tests, compatibility behavior, and documentation.
  No “implement now, verify in a later PR” steps. The final rehearsal verifies
  integration rather than supplying missing component tests.
- Build on merged main. Each PR must compile and preserve existing available
  behavior without an unmerged sibling. New capabilities remain unavailable until
  their prerequisites actually work; do not advertise a prepared output or enable
  training just because its wire types exist.
- Use feature-owned modules and narrow constructors. Do not grow the existing
  2,000–3,600-line Workflow modules with another subsystem. Move only the code needed
  for the current task; avoid a general refactor or test-only production abstractions.
- Persistence and protocol changes include explicit version/migration or rejection
  behavior in that PR. Preserve existing history and checkpoints; an unsupported
  document must never become an empty replacement. Follow the existing
  [storage](../crates/backend/storage/schema/README.md) and
  [training protocol](../crates/backend/training/protocol/README.md) version policies.

A PR description should name: the previous behavior, the narrower behavior now
usable, task/dependency IDs, changed-line count, exact checks and their results,
and any native capability still unavailable. “Tests pass” without naming whether
real Spiral or only a fake boundary was exercised is insufficient.

## 2. What is already implemented

These are source findings, not freshly measured runtime claims.

| Area | Existing implementation and evidence | Planning consequence |
| --- | --- | --- |
| Dataset discovery and episodes | [Dataset storage](../crates/backend/storage/src/datasets.rs) reads the project journal and resource/version metadata. [Episode browser](../crates/frontend/src/activities/data/episode_browser.rs) and [topic inspector](../crates/frontend/src/activities/data/episode_topics.rs) provide paginated episode → topic navigation. | Extend these paths; no import or catalog rebuild. Actual fixture population still needs inspection. |
| Exact episode selection | [Episode-filter notes](mcap-episode-filter.md) document the upstream UUID comparison fix in Spiral PR #7033. Storage binds an Arrow UUID for exact topic/video reads. | Revalidate the installed driver; do not budget a new workaround or describe the old rejection as necessarily current. |
| Recorded video | [Storage playback](../crates/backend/storage/src/datasets/playback.rs), [browser decoder](../crates/frontend/src/activities/data/episode_decoder.rs), and [player](../crates/frontend/src/activities/data/episode_playback.rs) implement bounded H.264 packet reads, WebCodecs, keyframe seek, cancellation, and a single-camera clock. [Playback notes](episode-playback.md) describe limits and tests. | Multi-camera ownership/synchronization remains work. Browser pixels do not prove native inference/training decoding. |
| Joint inspection | [Episode cache](../crates/frontend/src/activities/data/episode_cache.rs) and shared Timeline contain sample/trace machinery. [Recorded inspection](../crates/backend/src/data/queries.rs) is tied to Studio recordings; [MCAP reader](../crates/backend/storage/src/episodes/mcap_read.rs) expects Studio headers and looks up a recording filename through `dataset_source`. | Imported native episodes need exact dataset/member/topic projection. Reuse rendering; do not route native identities through the filename-based replay reader. |
| Workflow and Assistant | [Common contracts](../crates/common/src/workflow.rs), [manager](../crates/backend/src/workflow/manager.rs), [agent](../crates/backend/src/workflow/agent.rs), and [UI](../crates/frontend/src/activities/workflow/workspace.rs) already save revisions, validate relation-first graphs, show run results, and maintain durable conversations. | Adapt the existing authoring and run lifecycle. `run_workflow` currently lets the Assistant start Model work; explicit preparation needs enforcement at the backend. |
| Workflow identities | `DEFAULT_WORKFLOW_ID`, the manager's current graph, the agent's `set_workflow`, and [document persistence](../crates/backend/storage/src/workflow_documents.rs) effectively serve one active saved workflow. Graph revision history and multiple Assistant conversations are not a named-workflow catalog. | Add minimal named workflow selection before Training offers a workflow picker. Keep execution serialized; no concurrent multi-workflow scheduler. |
| Model execution | [Model runner](../crates/backend/src/workflow/model.rs) already runs pinned SigLIP, scans candidate keys, computes missing cells, reports progress/quarantine, and publishes cache plus registry transactionally. Tests cover cache reuse and publication/cancellation ordering. | Extend native inputs and provenance rather than build another model runner. Existing behavior is SigLIP scoring, not detection or task-success validation. |
| Feature identity and bounds | [Feature schema](../crates/backend/storage/src/workflow_features.rs) uses legacy five-part frame keys and source-relation identity; producer provenance is fixed to SigLIP. Model cache is bounded to 1,000,000 rows / 256 columns. Query previews are capped at 1,000 rows. | Add source-version identity and native observation keys; keep preview bounds distinct from full membership. Exceeding preparation limits must fail explicitly, never truncate a dataset. |
| Immutable outputs | Catalog versions and database transaction primitives exist, but current Workflow persists a graph/run preview and replaces mutable feature-cache relations. | A prepared output with pinned dependencies and complete episode membership is new. Historical reads and atomic visibility need native proof. |
| Training | [Rust training lifecycle](../crates/backend/src/training/manager.rs), [job protocol](../crates/backend/training/src/job.rs), and [Python jobs](../training/src/spiral_training_runtime/jobs.py) exist, as do ACT, telemetry, and checkpoints. [McapTrainingDataset](../training/src/spiral_training_runtime/mcap.py) is a 16-line class whose constructor raises an unavailable error; [policy readiness](../crates/common/src/training.rs) blocks MCAP training. | Implement the missing native dataset reader/loader and prepared input contract. Do not rebuild ACT or merely remove the blocker. |
| Worker ownership | [Training README](../training/README.md) and job code assign Spiral reads, batch construction, optimization, and checkpoint writes to the worker. Arrow batch transport is used for rollout inference; there is no public training-batch upload API. | Preserve worker-owned training. A prepared-input preflight can be added, but do not introduce a Studio-to-worker batch-upload architecture. |
| Verification | [.github/workflows/ci.yml](../.github/workflows/ci.yml) runs Rust, WASM/browser, formatting, and Python source checks. Native integration tests are ignored/opt-in. [Python tests](../training/tests/test_runtime.py) include mocked job lifecycle and a real ACT round trip. | Python execution and real Spiral checks need explicit commands/results per affected PR; default CI alone does not prove the training data path. |

## 3. Milestone map

| Milestone | Mergeable result | Tasks |
| --- | --- | --- |
| M0 — Establish native contracts | Repeatable capability evidence and a small fixture; missing upstream features identified separately. | 0.1–0.3 |
| M1 — Inspect synchronized episodes | Multiple recorded cameras and joint traces share one episode clock. | 1.1–1.4 |
| M2 — Author episode workflows | Named drafts; agent-owned interpretation; explicit preparation boundary. | 2.1–2.5 |
| M3 — Prepare native derived columns | Versioned observations and episode summaries, using the existing model runner. | 3.1–3.4 |
| M4 — Inspect reusable workflow outputs | Complete immutable membership, preparation states, review, and reuse after edits. | 4.1–4.5 |
| M5 — Train from a workflow | Worker resolves a prepared output, loads valid batches, and writes traceable checkpoints. | 5.1–5.6 |
| M6 — Verify the complete demonstration | Repeatable end-to-end evidence and presenter setup. | 6.1 |
| Optional Q — Clarification UI | Agent questions refine the same draft. | Q.1–Q.2 |
| Optional B — Bowl predicate | A validated visual producer and episode outcome, if suitable data/model exist. | B.1–B.3 |

There are **28 core PR-sized tasks**, plus five optional tasks. This is a review
breakdown, not 28,000 lines of promised new code: reuse and deletions count, and
small tasks should stay small. Re-estimate M3/M5 after M0; upstream implementation
is outside these Studio estimates.

The dependency structure is deliberately not one long chain:

```text
0.1 → M1: episode inspection ───────────────────────────→ 4.4: evidence review
0.1 → M2: workflow authoring ───────┐
0.3 → 3.1: source snapshots ───────┼→ M3 → M4 → M5 → M6
0.2: native image contract ────────┘           ↑
                                      worker image proof
M2 → Q: optional questions
M3 + suitable data/model → B: optional bowl predicate
```

M1 and most of M2 can land while native image or historical-query work is blocked.
M4 is the useful preparation/inspection milestone. **M5 is required for the user's
training goal**; finishing M4 does not complete the requested flow.

## 4. M0 — Establish native contracts

These are bounded probes and regression fixtures, not permission to patch Spiral
from Studio. A missing capability produces a precise upstream requirement and
blocks only dependent tasks. Ask the user before any workaround or scope change.
Do not invent SDK calls or pin an unverified version in this plan.

### 0.1 — Qualify a small native episode fixture

**Depends on:** none. **Size:** 400–700 lines.

- Extend the existing `native_recording_import_retry_playback_and_delete` integration
  fixture and exact-member tests. Cover two cameras, a late-start camera, a missing
  camera, joint/action topics, and empty topics. Include a representative imported
  schema from the real archive rather than only Studio's private recording format.
- Record driver build/hash, source versions, camera roles, joint/action field meanings,
  timestamp origins, current population, and test setup. Reuse the existing H.264
  seek fixture where suitable. Use unique disposable dataset names and explicit test
  configuration; do not reset the user's dataset.

**Merge result:** a reproducible input/capability record and regression fixture.
**Verify:** exact UUID isolation for two members and an absent member; channel
listing; packet ordering/keyframe seek; joint null/action preservation. Run the
native integration test on the qualified driver. Record unsupported schemas rather
than making up action targets.

### 0.2 — Prove native images for enrichment and the worker

**Depends on:** 0.1. **Size:** 500–900 lines, excluding upstream fixes.

- Probe supported native MCAP packet-to-image operations, stable observation identity,
  sampling, and timestamp behavior from the Rust backend and the Python worker's
  supported binding. Record availability separately for each consumer/platform.
- Verify small sequential and random-access reads against known fixture frames,
  including seeks requiring preceding keyframes and variable timestamp spacing.
  Identify supported pixel layout and preprocessing responsibilities.

**Merge result:** runnable native image contract checks, or a minimal failing
reproduction plus a documented upstream blocker. A failing upstream regression
must be opt-in with the exact reason, not a permanently failing default suite.
**Verify:** real image values, native keys/timestamps, bounded reads, and explicit
unsupported-codec behavior. WebCodecs success does not satisfy this gate. Neither
FFmpeg sidecar decoding nor a copied MP4 dataset is an acceptable bypass.

### 0.3 — Prove immutable reads and prepared-output visibility

**Depends on:** 0.1. **Size:** 600–1,000 lines, excluding upstream fixes.

- Exercise the native source snapshot/historical-query interface, resource retention,
  and transactions required to preserve derived columns plus membership. Extend the
  existing persistence/feature-publication tests rather than creating a new store.
- Use an explicitly selected disposable project; make the feature-cache integration
  fixture independent of implicit active Settings where needed. Document cleanup
  and recovery for native resources.

**Merge result:** supported operations and failure semantics are concrete enough to
implement M3/M4. **Verify:** advance a source, reopen the old version, and get the
same bytes/rows; inspect from a second connection before/after commit; test rollback,
conflicting publication, and reconciliation of an uncertain commit. Source retention
must be established, not inferred from retaining membership IDs alone.

## 5. M1 — Inspect synchronized episodes

### 1.1 — Make the existing camera follow an episode-owned clock

**Depends on:** 0.1. **Size:** 600–900 lines.

- Lift time/transport ownership out of the single camera in `episode_playback.rs`
  into the selected episode's inspection owner, with explicit props for camera
  time and readiness. Integrate the shared Timeline's existing transport surface.
- Reuse `EncodedCamera`/`DecodedGroup`; preserve keyframe seek, pause-on-hide,
  precision, and decoder cleanup. Retire the duplicate per-camera transport.

**Merge result:** current single-camera inspection runs through the episode clock.
**Verify:** existing WebCodecs seek comparison plus pause/resume, start/end bounds,
backward seeks, and episode switching. No visible regression or second clock.

### 1.2 — Display multiple recorded cameras together

**Depends on:** 1.1. **Size:** 800–1,100 lines.

- Open recorded camera panels from discovered topics under the selected episode;
  use the native workbench panel mechanism. Retain topic tables and breadcrumbs.
- Coordinate per-camera loading/readiness with the shared clock, bound aggregate
  encoded/decoded memory, and show coverage gaps individually. Moving a panel must
  not duplicate its media owner. Define how an unavailable camera affects transport.

**Merge result:** two cameras can be viewed and scrubbed together.
**Verify:** different frame rates/start times, one missing/failed stream, backward
seek across keyframes, panel close/hide, and rapid episode changes. Assert displayed
frame times against the playhead and that no previous episode frame survives.

### 1.3 — Read native joint/action samples by member and topic

**Depends on:** 0.1. **Size:** 800–1,100 lines.

- Add a bounded native sample projection under dataset storage and its HTTP contract,
  using `EpisodeSelection` and exact topic reads. Support the fixture's actual schema
  with named fields, timestamps, units, nulls, and recorded action channels.
- Reuse existing inspection value types where semantically valid; do not require
  Studio-private headers, current robot calibration, or a filename lookup. Return
  explicit unsupported-schema and unavailable-channel results.

**Merge result:** a usable read endpoint for native episode traces, independently
of camera UI. **Verify:** known joint values/times across two UUID members, optional
channels, repeated timestamps under the chosen ordering, gaps, units, and read bounds.
A 100-message topic preview must not masquerade as a complete joint trajectory.

### 1.4 — Connect joint traces to the shared inspection Timeline

**Depends on:** 1.1, 1.3. **Size:** 600–1,000 lines.

- Connect the native sample projection to existing trace rendering and current-time
  values. Keep state with the episode owner; use original units and explicit gaps.
- Make camera and joint evidence coexist in the same Data workspace. Preserve the
  distinction between joint observations and recorded action targets.

**Merge result:** synchronized camera/joint inspection. **Verify:** source-derived
alignment tolerances, a null interval, missing actions, and repeated episode switches.
Check mouse/keyboard use at 1024 pixels. No physical robot connection is required.

**M1 exit:** 1.2 and 1.4 work together on the same real episode fixture.

## 6. M2 — Author episode workflows

### 2.1 — Enforce explicit preparation at the execution boundary

**Depends on:** none. **Size:** 700–1,000 lines.

- Distinguish read-only result evaluation from explicit Model preparation in command
  admission. Update the existing manual model-running action to communicate
  **Prepare dataset**; the Assistant's authoring tools cannot authorize that work.
- Reject Model execution from `run_workflow` during ordinary authoring, including
  indirect graph paths. Preserve read-only query workflows and cancellation.
  Existing model completion remains a run receipt until M4 adds prepared outputs.

**Merge result:** drafting cannot accidentally launch enrichment; explicit manual
preparation still invokes the existing runner. **Verify:** simulated provider tool
calls cannot start a model; a user preparation command can; query-only runs and
stale-revision rejection still work. Enforce this in backend admission, not only
button state or prompt wording.

### 2.2 — Persist episode-source and interpretation contracts

**Depends on:** 0.1, 2.1. **Size:** 800–1,100 lines.

- Extend saved definitions with the chosen native dataset, camera/topic mappings,
  episode selection grain, required derived outputs, and a readable interpretation.
  Keep predicates/aggregation in the existing validated executable plan; avoid a
  second query language or free-form unvalidated execution instructions.
- Validate against real discovered source metadata and explicit producer support.
  Save unresolved drafts as non-executable with specific issues. Add the matching
  workflow-document compatibility handling and show interpretation in the existing
  graph/details view.

**Merge result:** a native episode workflow draft can be saved, reopened, and
inspected without claiming that its columns exist. **Verify:** round trip, unknown
versions, invalid topic/source identity, missing action versus inspection validity,
and whole-episode output requirements. Tests use real field names from 0.1.

### 2.3 — Store and address named workflows

**Depends on:** 2.2. **Size:** 800–1,100 lines.

- Extend Workflow document persistence and manager commands to create, list, load,
  and rename stable workflow identities with independent revisions/history. Migrate
  the existing default graph without dropping its history or Assistant association.
- Keep one admitted run at a time. Reject identity/revision conflicts explicitly;
  selecting another workflow must not retarget a running command.

**Merge result:** named workflows are usable through the API and survive reconnect.
**Verify:** create two, edit/reload independently, migrate the existing v3 history
fixture, reject cross-workflow stale saves, and preserve an active run's identity.
Deletion, folders, and concurrent workflow scheduling are outside this task.

### 2.4 — Select workflows in the UI and bind Assistant context

**Depends on:** 2.3. **Size:** 700–1,100 lines.

- Add a compact workflow picker/create/rename interaction to the existing activity.
  Carry the selected workflow ID through client state, agent tools, and conversation
  association instead of hard-coding `DEFAULT_WORKFLOW_ID` in `set_workflow`.
- Reuse existing revision guards and history UI. Respect current busy-state admission
  rather than adding editing-during-execution behavior.

**Merge result:** the engineer can return to a named draft and refine it with its
Assistant context. **Verify:** switch between two workflows, reconnect, delayed
responses from the previous selection, and an agent save after a revision change.
No conversation may silently modify a different selected workflow.

### 2.5 — Generate native episode plans from vague instructions

**Depends on:** 2.4; native producer contract from 0.2 for executable model plans.
**Size:** 700–1,000 lines.

- Extend the existing bounded discovery tools with dataset/topic/coverage information
  and actual producer capabilities. Replace the legacy video-manifest assumptions in
  the agent prompt and tool schema with the verified native input contract.
- Teach agent-owned camera mapping, scoring/aggregation, thresholds, unknown policy,
  and interpretation summaries. Reuse ordinary chat for corrections; the question UI
  is optional. The agent selects among supported producers, not arbitrary code.

**Merge result:** the dim-camera instruction yields a concrete, inspectable draft;
unsupported requests yield specific unresolved dependencies. **Verify:** deterministic
provider/tool fixtures for ambiguous wording, absent cameras, unsupported detection,
and a correction that changes the same draft. Also record one live provider trace
on the fixture; assess semantic validity rather than requiring identical SQL prose.
Assert zero full-enrichment calls during authoring.

**M2 exit:** natural-language authoring works with multiple saved drafts, and only
an explicit user preparation action can run enrichment.

## 7. M3 — Prepare native derived columns

### 3.1 — Resolve and read a pinned native source

**Depends on:** 0.3, 2.2. **Size:** 700–1,100 lines.

- Add a feature-owned source resolver using the proven native snapshot API. Bind the
  source dataset/dependency versions once, enumerate its exact episode membership,
  and propagate that identity through workflow input reads.
- Expose a bounded read/inspection of a pinned source through the existing Workflow
  input path. Preserve the reference for later prepared-output evidence reads.
  Display names and mutable `main` lookups cannot substitute for the pinned source.

**Merge result:** workflow source inspection can reopen a historical population.
**Verify:** same dataset name with a newer version, removed/inaccessible source,
and concurrent source advance during enumeration. Every returned member belongs to
the captured snapshot. No local version log or duplicated source storage.

### 3.2 — Version feature identity and reuse

**Depends on:** 3.1, native key contract from 0.2. **Size:** 800–1,100 lines.

- Extend the existing feature registry/cache identity with pinned source identity,
  actual native observation keys, producer revision, preprocessing, camera, and
  sampling semantics. Update registration/cache matching and schema compatibility
  together; legacy cells without sufficient provenance cannot count as compatible.
- Adopt these identities in the existing model request and cache lookup path,
  preserving server ownership of relation names and transactional registry writes.

**Merge result:** feature reuse is source-version-aware, exercised by the real
registration/read path. **Verify:** identical semantics reuse; same-name changed
source/model/prompt/sampling does not; two workflows can share compatible values;
old registry/history is preserved or explicitly rejected without destructive reset.

### 3.3 — Feed native images into the existing SigLIP runner

**Depends on:** 0.2 (Rust path passes), 3.2, 2.1. **Size:** 800–1,100 lines.

- Adapt native sampled observations into the pinned runner's image contract using
  the verified native decoder. Preserve native identity and time through inference.
  Keep image decode out of metadata-only planning/cache-hit paths.
- Separate an explicit sample preview from a complete preparation cohort. Reuse the
  existing bounded scan/backfill, progress, cancellation, and transaction machinery.
  If a configured bound is exceeded, fail rather than publish a truncated cohort.

**Merge result:** explicit preparation computes persistent scores on actual MCAP
frames. **Verify:** known fixture frame identities, a nonempty real model run, a
second run with zero model invocations, cancellation, and an over-bound cohort.
A synthetic image runner test alone is not the native integration proof.

### 3.4 — Materialize episode summaries and coverage

**Depends on:** 3.3. **Size:** 800–1,100 lines.

- Persist frame-observation completion/unknown/error distinctions and aggregate the
  agent's validated rule into episode-level columns with evidence times and coverage.
  Start with the supported lighting score interpretation; preserve its actual meaning.
- Expose summaries as derived output fields beside source episode identity. Account
  for every source episode, including those with no applicable camera observations.
  Decoding/execution failures block completion; computed unknowns follow the explicit
  policy. Do not treat quarantine counts as successful negative classifications.

**Merge result:** episode summary columns can be queried and audited independently
of the final prepared-output UI. **Verify:** dark fraction/temporal aggregation on
known observations, coverage denominators, unknown versus false, missing cameras,
and stable evidence timestamps. Test more rows than a result-preview page.

**M3 exit:** native derived columns persist with usable provenance and coverage.
This is not yet a fully prepared workflow output: M4 owns complete membership.

## 8. M4 — Inspect reusable workflow outputs

### 4.1 — Persist complete prepared outputs atomically

**Depends on:** 0.3, 3.2. **Size:** 800–1,100 lines.

- Add the prepared-output storage operation and read/list API using verified native
  primitives: immutable dependency references, complete episode membership,
  included/excluded/unknown decisions, counts, and evidence references.
- Make the completed output visible atomically. Derived resources may have earlier
  successful commits, but an incomplete output never acquires Prepared status.
  Include schema compatibility and uncertain-commit reconciliation by operation ID.

**Merge result:** the storage/API contract can publish and reopen a complete output
from supplied real fixture relations, before UI orchestration exists. **Verify:**
second-connection visibility, no partial result after failure/cancel, retry without
output duplication after uncertain commit, and exact old membership after source
advance. Test membership larger than the preview cap and unknown-policy outcomes.

### 4.2 — Complete preparation through the existing run lifecycle

**Depends on:** 3.4, 4.1, 2.3. **Size:** 800–1,100 lines.

- Connect explicit preparation to pinned source resolution, dependency backfill,
  episode summaries, full predicate evaluation, and atomic output completion.
  Retain the existing execution owner and cancellation/publication ordering.
- Record population/coverage/reuse counts independently of preview rows. Support
  reconnect and interrupted-run recovery; completed dependency work is reusable,
  while an incomplete output remains non-consumable. Preserve the last good output.

**Merge result:** one API action creates an inspectable prepared output. **Verify:**
end-to-end preparation through HTTP/service/storage; real source counts reconcile;
repeat runs reuse; cancellation/restart/stale requests cannot label an old or
partial result as current. No fake resumable progress or separate job scheduler.

### 4.3 — Show preparation readiness and progress

**Depends on:** 4.2, 2.4. **Size:** 700–1,000 lines.

- Bind the Workflow UI to Needs preparation, Preparing, Prepared, Failed/canceled,
  and Needs update states. Show the source population, intended producers, compatible
  reuse, and exact primary action from backend truth.
- Provide cancel/retry/reopen interactions and an identifiable older successful output.
  Keep “model columns exist” distinct from “this workflow output is complete.”

**Merge result:** the two-stage preparation workflow is usable through Studio.
**Verify:** UI transition fixtures plus one real prepare/reconnect cycle; progress
uses measured units, zero matches is a valid prepared result, and late responses
cannot overwrite a newer selected workflow. Check at 1024 pixels and keyboard-only.

### 4.4 — Review decisions and open their recorded evidence

**Depends on:** 4.3, 1.2, 1.4, 3.1. **Size:** 800–1,100 lines.

- Add paginated included/excluded/unknown results with derived values, reasons, and
  evidence times. Navigate to Data with the pinned source/output/member identities.
- Use the same multi-camera/joint inspector for evidence; extend its reads to the
  pinned source path where necessary. No copied preview membership or second player.

**Merge result:** every selected or excluded episode can be investigated against
its actual preparation evidence. **Verify:** overlapping exclusion reasons, page
counts, one unknown, source advance, unavailable history, and an old result opened
after another workflow/episode was selected. The original dataset remains intact.

### 4.5 — Re-evaluate edits without unnecessary model work

**Depends on:** 4.2, 4.3. **Size:** 700–1,100 lines.

- Compare dependency identities when a workflow changes. Predicate-only edits evaluate
  stored values and produce a new output; aggregation edits reuse sufficient stored
  observations; producer/sampling/source changes require affected preparation.
- Surface reuse/invalidation reasons and allow explicit selection of an older prepared
  revision. Extend source-update reuse only where immutable member identity proves
  compatibility; otherwise recomputation is the correct behavior.

**Merge result:** users can refine the selection and keep earlier prepared outputs.
**Verify:** the design doc's change/reuse matrix with producer-call counters, including
adding a feature, changing source bytes, insufficient observations, and two workflows
sharing compatible features. Old output membership and dependencies stay unchanged.

**M4 exit:** the primary instruction can be prepared, inspected, corrected, and
reopened without rerunning compatible enrichment. Preparation includes full membership.

## 9. M5 — Train from a prepared workflow

Preserve the current worker-owned data path. The native worker decoder must pass
0.2 before data loading is enabled. A mocked job lifecycle does not remove that gate.

### 5.1 — Add prepared-input and preflight wire contracts

**Depends on:** 4.1. **Size:** 800–1,100 lines.

- Carry a prepared-output reference, source/dependency identities, mappings, and
  sampling/split configuration through common training config, Rust job protocol,
  Python request validation, and canonical fixtures.
- Define a read-only preflight request/result for resolved input compatibility and
  window counts, since the current worker has no input-preflight endpoint. Add
  truthful capability negotiation; parsing support alone does not advertise loading.
- Version the protocol according to actual compatibility. Update existing request
  construction and rejection paths so the merged producer/consumer agree.

**Merge result:** the shipped boundary can express and reject/accept the new contract
correctly, with execution still unavailable until implemented. **Verify:** Rust/Python
canonical fixture round trips, incompatible runtime rejection, source-reference
validation, and omission of credentials from snapshots/logs. Old history remains readable.

### 5.2 — Resolve the worker's pinned episode source

**Depends on:** 5.1, 3.1; 0.2 worker binding and 0.3 historical reads pass.
**Size:** 800–1,100 lines.

- Add the worker's native Spiral connection/source reader and prepared-output resolver,
  using the verified supported binding and exact membership. Resolve camera/state/action
  schemas, source versions, field ordering, and units without reevaluating enrichment.
- Implement the preflight resolution stage and expose real mapping errors through
  the worker/backend adapter. Preserve the constructor's training blocker until
  valid window and batch construction are present.

**Merge result:** read-only preflight resolves a real prepared source and explains
incompatibility. **Verify:** stale/incomplete/foreign-project output, absent action
fields, mixed camera contracts, unavailable historical source, and source advance.
Use a real native project; output rows must match pinned membership exactly.

### 5.3 — Build valid training windows, splits, and statistics

**Depends on:** 5.2. **Size:** 800–1,100 lines.

- Add worker-owned valid-window selection with declared clock alignment, context and
  action horizon, camera coverage, null/gap rejection, deterministic whole-episode
  splits and sampler seed, and training-only normalization statistics.
- Complete preflight counts and bounded window-identity evidence. Record any additional
  training compatibility exclusions separately from workflow exclusions. Handle
  insufficient train/validation population explicitly.

**Merge result:** preflight reports the exact eligible training population and
configuration before any optimization. **Verify:** irregular timestamps, boundaries,
missing frames/actions, no cross-episode windows, no train/evaluation leakage,
fixed membership/order for a repeated seed, and statistics unaffected by evaluation
values. Pure sampler tests are paired with native fixture resolution.

### 5.4 — Load native camera/joint batches into ACT

**Depends on:** 5.3; 0.2 worker image decoding passes. **Size:** 800–1,100 lines.

- Implement the missing dataset batch interface consumed by `jobs.py` using native
  image reads, aligned state/action arrays, and the existing ACT preprocessing.
  Bound prefetch and memory; propagate load/cancel failures through the job lifecycle.
- Validate actual tensor shapes, camera ordering, and normalization with the existing
  ACT session. Keep public training admission blocked until durable checkpoint
  publication in 5.5 is complete.

**Merge result:** the real dataset loader produces verified ACT-compatible batches,
exercised directly and through a bounded worker integration harness. **Verify:** inspect
known pixels/state/action values and window IDs, run a real short CPU optimization
with a fixed test configuration, and assert zero workflow-enrichment invocations.
Do not add a public training-batch upload API or a local video-copy loader.

### 5.5 — Persist checkpoints with prepared-input lineage

**Depends on:** 5.4. **Size:** 700–1,100 lines.

- Implement the dataset checkpoint-write boundary used by the existing worker job;
  include prepared output, source/feature/workflow revisions, resolved windows/splits,
  sampler configuration, and normalization provenance in checkpoint/run records.
- Preserve current ACT artifact/tensor semantics and existing checkpoint readers.
  Add compatibility handling where provenance changes the persisted contract. Enable
  worker job admission only when source, loader, and publication capabilities pass.

**Merge result:** a worker API job can train from a prepared output and durably
publish a traceable checkpoint. **Verify:** real source → batch → optimizer → native
checkpoint → reload round trip; source/workflow edits do not change that lineage;
persistence failure is not reported as a successful checkpoint. Keep the existing
ACT checkpoint/inference round-trip regression.

### 5.6 — Choose a workflow and start training in Studio

**Depends on:** 5.5, 4.4, 4.5, 2.4. **Size:** 800–1,100 lines.

- Add Workflow/prepared-revision selection to Training and **Use in Training** to
  Workflow. Carry explicit references through activity navigation and call worker
  preflight for mappings, eligible counts, compatibility, and recovery actions.
- Replace the unconditional MCAP blocker with real capability/readiness checks.
  Preserve run history, stop, reconnect, and telemetry. Incomplete/updated drafts
  route back to preparation; never substitute raw source episodes or launch enrichment.

**Merge result:** the complete requested training flow is accessible in Studio.
**Verify:** successful short run, unprepared/no-match/missing-action/runtime-unavailable
cases, explicit use of an old prepared revision, and an edit after Start that cannot
retarget the run. Inspect its checkpoint lineage and measured progress.

**M5 exit:** a real checkpoint traces to the same prepared workflow output inspected
in Data; training input resolution and batch loading invoke no enrichment models.

## 10. M6 — Verify the complete demonstration

### 6.1 — Add the cross-activity regression and rehearsal record

**Depends on:** M1–M5. **Size:** 400–900 lines plus measured evidence.

- Add a focused integration scenario joining already tested boundaries: inspect two
  episodes → agent draft → explicit preparation → threshold-only edit → reopen old
  output → train → inspect checkpoint lineage.
- Record deterministic setup/cleanup, actual driver/model/runtime versions, observed
  timings/counts, and any clearly labeled prior run used in the presentation.
  Provide a second-operator walkthrough at 1024 pixels and presentation width.

**Merge result:** the complete demo is repeatable without source edits or guessed
state. **Verify:** run the scenario on the real native fixture, then repeat after
reconnect/source advance. Exercise a cancellation and unavailable service. Report
cold/warm timings and no unsupported policy-improvement or scale claims.

## 11. Optional milestones

### Q.1 — Persist and resume agent clarification questions

**Depends on:** 2.5. **Size:** 700–1,100 lines.

Add a typed agent question/answer operation with suggested answers, free text,
optional explicit delegation, pending state, and conversation/workflow revision
identity. Continue independent tool work but do not apply dependent edits before
a required answer. Release active-turn resources while waiting; an unanswered
question must not indefinitely hold a catalog lease or require a live provider
request. Resume from durable conversation state.

**Merge result:** the API can ask, answer, and resume on the same draft.
**Verify:** reconnect, cancel, stale answer, wrong conversation, optional delegation,
and a resumed definition change. Questions and answers never invoke preparation.

### Q.2 — Render clarification inside the existing Assistant

**Depends on:** Q.1. **Size:** 600–1,000 lines.

Render pending/answered questions beside the visible draft, suggested responses,
free text, and **Use your judgment** where allowed. Explain the consequence of the
choice and show the resulting interpretation change. Reuse ordinary chat/history.

**Merge result:** an agent-initiated question can refine a workflow in Studio.
**Verify:** keyboard submission, suggested/free-text answers, reconnect with pending
question, and no implicit answer from a preselected option. If M4 is available,
verify that an answer invalidates only the affected prepared dependencies.

### B.1 — Qualify the bowl task and one producer

**Depends on:** 0.1, 0.2, 2.5. **Size:** 300–700 lines plus evaluation evidence.

Review actual success/failure/occlusion clips and test one supported visual producer
against the agent's stated task interpretation. Document required native operations,
model artifact/version, output grain, limitations, and representative counterexamples.
Do not treat SigLIP as object detection or object boxes alone as task success.

**Merge result:** a concrete producer contract/evaluation fixture, or a clear blocker.
**Verify:** human-reviewed evidence and actual model outputs; no fabricated accuracy
claim. Unknown upstream/model integration work is re-estimated after this gate.

### B.2 — Add that producer and persist its native outputs

**Depends on:** B.1, 3.3. **Size:** 800–1,100 lines; split if the proven producer
needs more, rather than hiding a new model framework inside this PR.

Add the narrow producer adapter and typed outputs to existing execution, feature
identity, capability discovery, and persistence. Preserve existing SigLIP behavior.

**Merge result:** explicit preparation can compute reusable outputs for that producer.
**Verify:** native input/output identity, pinned artifact, reuse/invalidation, missing
objects, and cancellation. Introduce no general model marketplace or execution engine.

### B.3 — Aggregate and review bowl outcomes

**Depends on:** B.2, 3.4, 4.4, 4.5. **Size:** 700–1,100 lines.

Apply the agent's episode-level temporal interpretation to stored observations,
compose it with lighting, and expose reasons/evidence in existing review. Reuse
compatible lighting columns. Prefer existing summary/predicate mechanisms; implement
only missing temporal semantics and the evidence presentation they require.

**Merge result:** the optional second predicate participates in the same prepared
workflow and training membership. **Verify:** success/failure/occlusion examples,
compound conditions, reuse counts, and a definition change from “ever inside” to
“inside at the end.” Q is optional; the agent can decide and explain without it.

## 12. Verification lanes and sequencing rules

Use the existing test owners and tooling. The commands below describe execution
for future PRs; they were not run during this documentation change.

| Lane | Existing entry point | Required for |
| --- | --- | --- |
| Rust contracts/service/storage | `nix develop .#local-driver --command cargo test -p spiral-robotics-common -p spiral-robotics-storage -p spiral-robotics-backend --lib --locked` (narrow by task/test while iterating) | Changed Rust contracts and execution boundaries. |
| Native dataset | `native_recording_import_retry_playback_and_delete` in dataset recording import, using explicit `SPIRAL_MCAP_TEST_CONFIG` and `--ignored` | Native member/topic/packet and fixture changes. |
| Native persistence | `adbc_persistence_contract`, `adbc_transactional_ingestion_stays_isolated`, and `feature_cache_and_registry_round_trip_in_one_spiral_transaction` | Schema, feature identity, atomic output, and historical-read work; add focused regressions alongside these owners. |
| Browser | Existing WASM test runner in CI; `independent_keyframe_decode_matches_continuous_decode` plus episode clock tests | Playback, lifecycle, precision, and synchronization. Confirm the decoder actually executes in the selected browser; a skipped codec test is not proof. |
| Training boundary | `cargo test -p spiral-robotics-training --locked`; Rust/Python canonical wire fixtures | Protocol and request/result changes. |
| Python execution | From `training/`: `uv run pytest` with focused selections while iterating | All worker changes. Run real native data integration in addition to mocked job tests. |
| Static/build checks | Existing CI formatting, Clippy, native/web builds, Ruff, and packaged-version checks | Every relevant PR. Default CI currently does not execute the Python runtime suite. |
| Live semantic/product checks | Versioned native fixture, one real provider trace, real model output, short ACT job, desktop walkthrough | Milestone exits; supplement deterministic tests without making provider wording a flaky assertion. |

Before merging a task, run the relevant lane and attach its result. Native tests
need an explicit disposable project and qualified driver. Do not run all ignored
tests indiscriminately; the repository documents an unresolved rollback race, and
some older integration tests consult active project settings.

Prefer this initial merge order: **0.1, 2.1, then 1.1/1.3 and 2.2**, while 0.2 and
0.3 qualify their separate native dependencies. Continue by task dependency, not
by waiting for every numbered milestone to finish. Q can be scheduled once M2
works. B is conditional on its producer/data proof. Neither blocks the dim-camera
preparation-to-training demonstration.
