# Spiral robotics demo plan

Status: **draft for iteration**, September 9, 2026. No implementation is authorized
by this document. Proposed choices below are recommendations, not settled product
decisions. Repository review: `24ab008` plus the existing working-tree changes.

## 1. Goal and reference

Build a working Studio demonstration for an ML engineer who wants to understand
robot behavior and decide which data a policy should learn from. The central proof
should be a traceable connection between something seen in a recording and the
actual training examples selected because of it.

The [Spiral homepage](https://spiraldb.com/) presents a synchronized robot/video
hero, then Investigate → Enrich → Curate → Train, followed by a version-history
story. Its examples combine visual evidence, telemetry, selection boundaries,
and reproducible training inputs. Treat these as product ambitions and presentation
references, not verification of APIs or performance in our installed driver.
The [Deep Dives page](https://spiraldb.com/deep-dives) adds a useful technical
theme: account for how a logical request becomes physical reads and tensors.
Both were read on September 9, 2026.

**Working interpretation:** build the homepage's robotics workflow inside Studio.
A standalone marketing animation or the separate MP4 deep-dive interactive would
be a different deliverable. Preserve that distinction when we iterate.

Recommended headline for our demo: **Inspect a failed attempt, define a useful
training selection, and reproduce exactly what changed.** Whether “failed grasp”
is the right concrete example depends on the recordings we actually have.

## 2. Scope in two deliveries

**First useful demo:** one real native MCAP dataset; synchronized inspection;
one clearly defined behavior signal; a population query; review of matches and
counterexamples; and a saved selection whose provenance can be reopened.
Target a presenter-led 5–7 minute walkthrough. This is a proposed format.

**Complete target:** immutable published selections, a visible membership diff,
native snapshot replay, and ACT training that consumes precisely the selected
windows. Finish with evidence connecting a checkpoint to those inputs. A measured
policy comparison is a later experiment, not a promised consequence of curation.

Do not call the first delivery the complete training loop. If immutable selection
publication is blocked, it is an investigation preview, not a reproducibility demo.

Keep these out of the first delivery:

- New robot support, physical motion during the walkthrough, or Live MCAP capture.
- Multiple enrichment models, a general annotation platform, and arbitrary graph editing.
- Multiplayer editing, branch merge UI, automatic mixture optimization, and distributed training.
- New import formats, a new activity/navigation system, or a separate demo-only application.
- A full project-backup rewrite. Native dataset recovery requirements still need
  to be satisfied before using a disposable demonstration project.

## 3. What we can build on—and what we must verify

This is a source/document review, not a live integration test. Recent migration
notes are more specific than older “shipped today” statements in PRODUCT.md.
In particular, legacy episode/video/training support does not establish support
for the project's migrated native MCAP dataset.

| Area | Evidence in this checkout | Consequence for the demo |
| --- | --- | --- |
| Native data | Migration notes report 52 episodes, 10 topics, and 66,549 decoded messages; current catalog name is `dataset_default` | Useful initial fixture candidate. Recheck current population and inspect content before choosing the story. |
| Episode inspection | Dataset/episode/topic UI exists. Working-tree code adds parameterized episode-topic preview. September 8 notes still report UUIDv7/generic UUID equality rejection | Wiring is not evidence of a successful driver round trip. Exact member filtering is the first gate. |
| Playback | Shared Timeline, camera, and recorded-source infrastructure exists; migration notes explicitly leave native MCAP playback unadapted | Reuse the existing surface, but implement and verify its native data path. |
| Workflow | Relation-first Query/Model/Result flow, pinned SigLIP scoring, feature registry/cache, and optional Assistant exist | Adapt native topic/frame queries and stable identities. Existing saved workflows may reference removed data. |
| Behavioral signals | SigLIP provides image/concept scores | This is not temporal failure detection, tracking, calibrated force, or metric slip measurement. |
| Curation | Query results and workflow persistence exist; PRODUCT.md lists derived curation as future work | A saved SQL draft or row-limited result is not a published training selection. |
| Versions | Native catalog reads resource versions; migration tooling uses journal commits and immutable resource successors | Useful native foundation. Historical query execution and atomic selection publication still need focused proofs. |
| Training | ACT configuration, remote compute protocol, telemetry, and checkpoint storage exist; current contracts still carry the legacy dataset selection | Native MCAP membership, aligned windows, and immutable input selection need integration. |
| Recovery | Migration has verified MCAP backups; fixed-table project bundles do not automatically cover native datasets | Do not assume Settings backup can reset the proposed demo project. |
| Driver | README and migration docs require the local driver for current native MCAP functions | Record and test the actual driver build; the default published pin is insufficient evidence. |

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

## 4. Choose the data before the story

Start by reviewing the existing recordings. Choose a task with a visible outcome,
consistent camera coverage, usable actions/state, and enough variation to explain
why a boundary includes one example and excludes another.

Proposed small review set: 12–20 episodes spanning clean success, unsuccessful
attempts, successful recovery, and visually confusing cases. These are desired
categories, not assertions about the current archive. Keep a separate held-out
set of whole episodes; never split adjacent frames from one episode between
training and evaluation.

Record the following evidence before selecting the hero episode:

| Evidence | Required decision |
| --- | --- |
| Native member identity and source revision | What recording and immutable source do we mean? |
| Camera topics, timestamps, state/action channels | Can all displayed streams share a defensible clock? |
| Missing intervals and coverage | Which moments cannot be scored or trained on? |
| Units and calibration | Which physical quantities are actually measured? |
| Human-reviewed outcome and interval | What makes the seed interesting, and what would falsify our interpretation? |
| Data permission and provenance | Can the chosen clips be shown to the intended audience? |

Prefer the smallest real archive that proves the behavior. Keep three distinct
counts throughout: episodes, selected intervals, and sampled training windows.
An enrichment score on 100 frames must not be presented as 100 episodes or as a
complete corpus scan.

If the archive lacks a convincing failure/recovery example, select a different
observable behavior or obtain suitable data as an explicit scope decision.
Do not invent force telemetry or infer millimeters from uncalibrated image/depth
outputs. Frame-level visual scores should retain that description.

## 5. Proposed walkthrough

| Beat | Presenter action | What the audience sees | Required proof |
| --- | --- | --- | --- |
| Orient · 30 s | Open the prepared dataset and seed episode | Actual dataset population and a recognizable task | Dataset/version and member identity are available in details. |
| Inspect · 60–90 s | Play, pause, scrub, and mark the interesting interval | Camera imagery and state/action traces agree at one playhead | Every panel reads the selected member; gaps and unavailable streams are explicit. |
| Explain · 60 s | Open the signal definition and inspect evidence around the seed | A concrete, readable rule or model concept with units/grain | Producer, inputs, coverage, and limitations can be inspected. |
| Expand · 60–90 s | Run the same condition across the prepared population | Bounded result previews with actual total counts, when available | Each match opens at its exact interval; previews do not silently define membership. |
| Review · 60–90 s | Inspect a positive, a counterexample, and a recovery; adjust the condition | The selection changes for an understandable reason | Unsupported/unknown values stay distinct from negative classifications. |
| Preserve · 45 s | Publish, reopen, then compare a deliberate revision | Exact membership and provenance, with added/removed intervals | Old membership survives source updates and reconnect. |
| Train · 60–90 s, complete target | Launch a short real run from the published selection | Resolved input contract, real batch progress, and checkpoint lineage | Training consumes the pinned selection, not a fresh query against current main. |

The focal moment is the counterexample: a convincing-looking match turns out to
be useful recovery behavior, and the selection is corrected using visible
evidence. This demonstrates judgment rather than treating every failure as data
to delete.

A failed attempt can be useful for inspection or evaluation without being a
positive behavior-cloning target. Before training, explicitly decide whether
selected failures are excluded, used only for evaluation, or contribute reviewed
recovery windows. Do not silently train ACT to imitate unsuccessful actions.

Long enrichment/training work can be prepared in advance with real persisted run
receipts, clearly labeled as previous runs. The walkthrough should still execute
a bounded live query, change a selection, and reopen its saved result. Preparation
must not become fabricated progress or cached answers to a different request.

## 6. Product and data contracts

These are conceptual responsibilities, not a proposal for one large Rust struct
or a new storage engine. Establish the native Spiral operations before choosing
the persistence schema.

| Concept | Identity and invariants |
| --- | --- |
| Source snapshot | Native project commit plus resource/version references for the complete dependency set. A display name is not identity. |
| Episode interval | Native dataset member plus integer start/end timestamps in a declared clock; use half-open intervals and keep original Studio IDs separate. |
| Signal definition | Name, semantic meaning, grain, units/null meaning, pinned producer, parameters, and input snapshot. A changed definition creates a distinct version. |
| Signal observations | Stable frame/window keys plus coverage and producer provenance. Missing computation differs from a computed false/low score. |
| Selection definition | Predicate, explicit review decisions, source/signal versions, and interval construction policy. |
| Published selection | Immutable resolved membership with counts/checks and dependency references. Publication is atomic; a partial run cannot appear published. |
| Training input | Published selection references plus sampling, context/horizon, camera/state/action mapping, normalization, seed, and split policy. |

Key semantic decisions:

- Decide whether the first selection contains whole episodes or intervals. Recommend
  intervals for the final target; a whole-episode first increment must say so.
- Convert frame scores into intervals through a documented temporal rule, with
  explicit handling of gaps, minimum duration, and adjacent matches. A single
  image score cannot establish a failure event by itself.
- Preserve original timestamp precision. Validate clock origins and offsets;
  do not join independent streams merely because their row numbers coincide.
- Training context and action horizons must stay inside valid same-episode data.
  Define whether context may extend outside a selected interval; never let this
  happen implicitly. Reject windows crossing missing data or split boundaries.
- Freeze any normalization statistics using training-only data and record their
  provenance. Freeze evaluation membership before adjusting the curation rule.
- Deduplicate overlapping windows or define their sampling multiplicity. Later
  mixtures need explicit weight semantics and overlap policy.
- A historical selection must pin source bytes/revisions as well as SQL text.
  A mutable external object reference alone cannot guarantee reproducibility.
- Reuse a feature cache only when source identity/version and producer semantics
  match. Existing source-name-based behavior needs review for refreshed datasets.

The first retrieval method should be an inspectable signal predicate over real
relations. Query-by-example embeddings, tracking, or metric slip detection are
separate extensions requiring validated native operations and quality checks.

## 7. Studio interaction direction

Mode: **Operate**. Preserve Studio's existing light, compact desktop workbench,
tokens, native panel mechanics, and mouse/keyboard interaction. Minimum viewport
is 1024 CSS pixels. No mobile layouts or touch-specific behavior.

Use existing activities:

- **Data:** choose the source/member, inspect synchronized streams, mark an interval,
  and return from a result to its evidence. Register content with the existing
  panel system and reuse the shared Timeline/camera/signal owners.
- **Workflow:** define candidates, compute one supported signal, and compose the
  selection query. Keep native relation lineage visible. The graph and run result
  remain authoritative; the Assistant is optional for the first walkthrough.
- **Selection review:** initially a panel within Data or Workflow, with predicate,
  evidence, coverage/checks, publish action, and version comparison. Exact home
  remains an iteration decision; avoid adding a sixth activity.
- **Training:** accept a published selection explicitly and show its resolved
  input contract before starting the existing training lifecycle.

The default investigation composition should put episode/results navigation at
left, camera and relevant signals in the main area, and one shared Timeline below.
Keep the evidence visible when changing a boundary. A compact predicate control
may complement SQL once semantics settle; do not build a second query language.

Cross-activity actions carry source version, member identity, and interval
explicitly. Preserve each activity's independent layout and local state. Do not
use ambient selection or panel IDs as data identity.

Required states include loading, no matches, unscored data, partial coverage,
decode failure, canceled computation, stale draft, publication conflict,
unavailable historical source, and unavailable training runtime. Preserve the
last successful result with its identity and age; never label it as a new result.
If a native operation fails, surface the responsible operation and recovery path.

## 8. Delivery sequence and acceptance gates

Work in vertical slices. Estimate effort after Gate 0 resolves the driver and
data contracts; calendar promises before that would conceal upstream dependency risk.

### Gate 0 — Establish the executable contract

- Inventory the actual dataset, saved Workflow references, source revisions, and
  driver/runtime versions. Refresh stale assumptions in this plan.
- Prove exact episode-ID parameter filtering for at least two distinct members,
  checking every returned row and an empty-result case.
- Prove bounded native camera extraction and a supported route to playback and
  decoded model inputs. Topic/protobuf inspection alone is insufficient.
- Prove the native historical-query context and atomic publication primitives
  needed to preserve a selection and its dependencies.
- Identify a safe demo project and a verified recovery method before data setup.

**Exit:** a small capability record with operation, version tested, observed
result, and unresolved dependency. For Spiral bugs/missing features, stop that
dependent work and ask the user about an upstream fix or an explicit scope change.
Do not add casts, alternate identity filters, a local catalog/version log, a fake
Dataset, or a second database to bypass the missing capability.

### Slice 1 — One trustworthy native episode

Connect native MCAP reads to the existing recorded-source projection and Timeline.
Add bounded loading and explicit gaps; handle H.264 random access from a suitable
keyframe using the supported media path.

**Exit:** two episodes can be alternately opened and scrubbed with no mixed
members or stale camera frames. Validate alignment against source timestamps;
define tolerances from the actual stream rates, not visual plausibility.

### Slice 2 — One signal and a reviewed population

Adapt candidate queries to native frame identities. Start with a meaningful
measured rule or the already-supported SigLIP concept score, depending on the
reviewed data. Run a bounded real enrichment, inspect coverage, and query matches.

**Exit:** seed, positives, false positives, and a recovery/counterexample are
reopenable at exact times. Repeating an identical computation demonstrates real
reuse; changing source/producer semantics cannot reuse incompatible cells.
Human review establishes whether the signal is useful, not just executable.

### Slice 3 — Publish and reproduce a selection

Add the domain publication operation and review panel on verified native version
primitives. Store complete membership and checks, independently of result preview
limits. Show meaningful added/removed intervals between two versions.

**Exit:** reopen after reconnect; change the predicate and advance the source;
the old selection still resolves to identical membership. Test concurrent/stale
publication and an uncertain commit outcome without duplicating publication.
This completes the first useful demo.

### Slice 4 — Train from native selected windows

Replace the legacy input assumption with explicit native selection resolution.
Reuse the existing Arrow/HTTP ACT runtime and checkpoint lifecycle. Start with
one worker and one selection; prove alignment before adding mixtures.

**Exit:** inspect a real batch, confirm all window identities belong to the
resolved selection and split, run a short optimization job, persist a checkpoint,
and trace it back to exact source/signal/selection versions. A rerun reproduces
input membership and sampler order under the declared configuration; do not
promise bitwise GPU/model reproducibility without separate evidence.

### Slice 5 — Rehearse, then extend

Prepare the presenter sequence, verified prior runs, capability/version record,
and a reproducible project setup. Rehearse at 1024 and a normal presentation
desktop width; cover keyboard access, canceled work, and unavailable services.

**Exit:** a second operator can run the sequence from the written setup without
editing source code or guessing missing state. Record actual query, decode,
enrichment, first-batch, and training timings, distinguishing cold and warm runs.

Only then consider multi-selection weighting, query-by-example search,
native detection/depth/tracking, richer journal UI, distributed throughput, and
a controlled policy-quality experiment. Keep each extension tied to a concrete
proof rather than adding illustrative metrics.

## 9. Validation and claims

The demo is ready when a viewer can follow this chain in the product:

```text
source version + episode + interval
    → signal evidence and coverage
    → reviewed selection version + exact membership
    → resolved training windows + sampler configuration
    → training run + checkpoint
```

Use targeted integration checks at the real boundaries: member isolation,
timestamp/media alignment, feature provenance, atomic publication, historical
membership, split leakage, and batch/checkpoint lineage. UI checks should exercise
those user-visible transitions. No broad refactor or unrelated test expansion is
required to begin.

Candidate responsiveness targets for the small prepared dataset are under two
seconds to first useful episode evidence and under two seconds for a warm bounded
query preview; confirm or revise after measurement. Scrubbing should keep controls
responsive while obsolete requests are canceled or ignored. Report enrichment
and training progress from measured work, with no invented ETA.

Do not claim corpus scale, GPU saturation, model improvement, or native SDK API
support from the website examples. If policy improvement becomes part of the
demo, compare baseline/curated runs with fixed evaluation episodes, documented
training budgets and seeds, and repeated measured trials. Training loss alone
does not establish better robot behavior.

## 10. Decisions for our next iteration

| Decision | Recommended starting point | Status |
| --- | --- | --- |
| Audience and format | Presenter-led Studio walkthrough for robotics ML engineers | Proposed |
| Reference scope | Homepage robotics workflow, including the synchronized inspection moment | Proposed |
| Initial corpus | Audit existing `dataset_default` recordings first | Proposed; content not reviewed |
| Behavioral story | Preserve useful recovery while identifying unhelpful attempts | Conditional on data |
| First stopping point | Published, reopenable selection after Slice 3 | Proposed |
| Training proof | Short real ACT run plus checkpoint/input lineage; quality study later | Proposed |
| Signal | One measured rule or supported visual concept; choose after data review | Open |
| Selection grain | Intervals for the target; explicitly decide any whole-episode increment | Open |
| Native version/publication contract | Prove supported Spiral primitives in Gate 0 | Open |
| Standalone visual recreation | Separate follow-on if wanted | Outside current proposal |

Recommended next discussion: settle the corpus/story and the first stopping
point. Then run Gate 0 and refine the plan around measured capabilities before
starting UI implementation.

## Iteration log

- **2026-09-09 · Draft 1:** read the live homepage and Deep Dives overview; inspect
  current Studio contracts and migration notes; propose the walkthrough, native
  capability gates, delivery slices, and acceptance evidence. No live project,
  driver, browser Studio, model, or training validation performed for this draft.
