# Demo: Train on your best demos

Drafted October 8, 2026. **Status: plan.** Nothing is recorded yet, and most of the loop isn't built. **Target: about 90 seconds, including the title and end cards.** Audience: customers and investors, as for the [tour](tour.md).

The demo closes the loop the other videos leave open. So far Score progress has used someone else's model. Here, the user picks their own expert demos with a rule, trains a progress model on them, publishes it, and Curation uses it:

```text
Dataset → Track objects ("bottle", 5 per second) → Select (duration per bottle, lowest 25%) → Publish expert_demos
        → Training (progress model on expert_demos) → Publish model "Bottle experts"
        → Curation: Score progress with "Bottle experts" → the hero's stall, left out → Data
```

It follows the [demo guide](guide.md). The hero and its turning bottle come from the [tour](tour.md#narration-and-screen-action) and [video 1](series/01-score-progress.md).

## The story

A progress model has to learn what good looks like, and that usually means someone labels expert demos by hand. In Spiral, you pick the experts with a rule instead. Demos carry two to six bottles, so raw time isn't fair. Track objects counts the bottles, and a Select keeps the fastest quarter by time per bottle. You publish them, train a progress model on them in Training, and publish the model. Back in Curation, Score progress uses your model, and it flags the same moment the tour shows: the left gripper turning a bottle instead of dropping it.

This is the WARP-RM paper's own recipe, which needed no manual labels. It trained on the shortest 25% of demos for each bottle count ([below](#the-expert-rule)).

## Narration and screen action (draft)

The lines are a first draft for the story, not timed. Beats marked **needs build** depend on [what's missing](#how-far-we-are).

**Opening: Data.** The full dataset, about 2,400 demos (**needs build**: the [full import](#how-far-we-are)). Open the hero and play the turning at 1×.

> These are about 2,400 demos of a robot clearing bottles into a bin. Some are quick and clean. Others drag.

> Watch the left gripper. It keeps turning the bottle instead of letting go.

> A progress model can catch moments like this, but first it has to learn what good looks like. So let's teach it with our best demos.

**Track the bottles.** Curation → Dataset + → **Track objects**, "bottle" on the top camera, 5 per second. The fill eases into a time-lapse, and an episode opens with every bottle boxed.

> Each demo has two to six bottles, so time alone isn't fair. Track objects counts them.

**Pick the experts** (**being built**). Track objects + → **Select** → **Duration per bottle**, **lowest 25%**. The strip shows where the cut lands (on the sample it's 7.2 s per bottle).

> Then I keep the fastest quarter, by time per bottle. Those are my experts.

**Publish** `expert_demos`. Secondary steps, fast.

> I publish them as a view.

**Train** (**needs build**). Training → view `expert_demos` → **Progress model** → Start. Uploading and training ease into a time-lapse of the loss curve.

> In Training, I train a progress model on that view. It runs on a cloud GPU, reading the clips from SpiralDB.

**Publish the model** (**needs build**). On the finished run, **Publish model** → name it "Bottle experts".

> When it's done, I publish the model.

**Use it** (**needs build**). Curation → a curation over the long demos → **Score progress**, scored by **Bottle experts** → **Select**, progress rate at least the cutoff. The hero's left-out stretch replays at 1×.

> Back in Curation, Score progress can use it. And it flags the same moment, the bottle turning in the gripper.

**Data.** Publish and open the view. End on the loaded view, then the end card.

> The demos you trust train the model that cleans up the rest.

Pacing: Track objects at full scale, the upload and the training are long waits, so each becomes a short eased time-lapse. Their answers and the trained model are stored, so later takes run fast, as in the tour.

## How far we are

Two of the eight steps work today: publishing a view and opening it in Data. Track objects counts bottles, but only on the 100-episode sample. Everything in between is missing.

| Step | Today | Missing | Size |
| --- | --- | --- | --- |
| 1. Dataset at scale | `warp_sim_bottles_sample`: 100 of the 2,438 source episodes. Its fastest quarter by time per bottle is **25 episodes, 13.1 min** of video. The paper trained on 610 episodes, 3.4 h. | Import all 2,438 episodes of `uynitsuj/sim-bottles-mjwarp-v1` (31 GB; the top camera, the only one WARP reads, is 9.2 GB). Native route: `spql.lerobot.dataset` (draft spiraldb#7688, now owned by someone else). The old MCAP converter was deleted. | M, plus import time |
| 2. Track bottles | Track objects (SAM 3.1): most bottles seen at once matches the recorded counts on 88 of 100, and the rest are off by one. 100 episodes take about 6 min on two H100s. | Being built: a sample-rate setting, and the tracks as the node's one column instead of a count and a rule. At full scale it's about 24× the sample's pictures (about 5 H100-hours, spread over containers). Check that the published view's fill reaches every episode, not just the 100-episode preview. | Run it |
| 3. Lowest quarter by duration per bottle | A Select compares four fixed episode facts (duration, messages, channels, size) with an absolute number (`document/selection.rs:43`). It can only follow the Dataset node (`document.rs:252`). | Being built, as part of the [Curation refactor](#the-curation-refactor): a Select after any node, "most bottles at once" and "duration per bottle" computed from the tracks, and "lowest N%" as a percentile rank. | M |
| 4. Publish the experts | Works. The view name is the label. | — | — |
| 5. Train a progress model | Training is generic in its plumbing but ACT at every layer of its contract. `TrainingPolicy` has one variant (`crates/common/src/training.rs:11`), `TrainingConfig.act` is required, the Python request accepts only `"act"`, and admission needs joint roles with commanded positions (`crates/backend/src/training/input.rs:60`). | A second training family: config, request fixture, admission without joints, and a Modal job that trains the WARP-RM head. Upstream `uynitsuj/WARP-RM` (MIT) has the trainer. Use it pinned rather than porting it, and feed it frames from our camera streams with the scorer's preprocessing (`modal/warp_progress` `SquashPreprocessing`), not Training's PyAV resize. | L, the critical path |
| 6. Publish the model | Checkpoints sit on the Modal Volume as `(run, step)`. No model registry exists anywhere. | **Publish model** on a checkpoint, kept as a Studio record (name → run, step, camera), the same way curated views are. The weights stay on the training Volume. | S–M |
| 7. Score progress with it | The wire already carries the model: `AnswerSource::ProgressService{camera, model, revision}`, and the answer column's hash includes the model, so two models never share a column. | `ProgressScoring {}` has nowhere to store a choice (`document.rs:371`), and `ProgressScorer` has one variant. `warp_progress` bakes the released head into its image (`app.py:35`) and answers 422 to any other model. **Studio stores that 422 as a failed answer and never asks again** (`crates/backend/src/curation/fill.rs:281`). The service has to load heads from the training Volume by model, and an unknown model must not poison the column. The picker shows published names, never model ids. | M |
| 8. Open it in Data | Works. | — | — |

Reused as is: stream upload and content addressing, the Modal control service and Volume, stop and resume, `history.json` with the loss and performance charts, the `ModelArtifact` envelope, and the answers-column plumbing.

### The expert rule

- **Paper:** the shortest 25% within each bottle count, using the dataset's `meta/object_counts.json`.
- **This demo:** the fastest 25% by time per bottle, with the counts from Track objects, so no labels at all.
- **On the 100-episode sample** (recorded counts, not Track objects):
  - The cut is 7.2 s per bottle; the median is 7.9 s.
  - The rule leans toward bigger loads. Of the 25 experts, 9 have 6 bottles and 3 have 2, because each demo has a fixed setup and finish.
  - 19 of the 25 are also in the paper's per-count pick.

### The recipe the trainer has to match

From the paper and the upstream README:
- **Backbone:** frozen DINOv3 ViT-B/16, with its features cached.
- **Head:** a 12-layer temporal transformer over 32-frame windows at a 15-frame stride, which is what `warp_progress` already runs.
- **Augmentation:** time-warp only (log-velocity AR(1), reversals).
- **Loss:** cross-entropy against a two-hot target over 30 bins.
- **Optimizer:** AdamW at 4e-4, batch 1024 windows, 15k steps.
- **Text:** none. It's one model per task.

The paper doesn't give GPU time. We measure it on the first real runs.

## The Curation refactor

Decided October 8. Model nodes only add columns, and Select is the one filter. `docs/curation.md` is being rewritten to match.

- **One row per episode, always.** The Dataset starts the table with the episode's facts and `kept`, its kept time ranges, which start as the whole episode.
- **Model nodes only add columns,** one for each thing their settings name, and have no rule:
  - **Track objects:** a timeline of pictures, each `{timestamp_ns, objects}`, at the rate it's set to.
  - **Score progress:** a timeline `{timestamp_ns, progress_rate}`.
  - **Break down:** its subtasks `{description, start_ns, end_ns}`.
  - **Find similar:** every subtask with its similarity.
  - **Group similar:** one range list per category.
- **Select follows any node and filters on anything upstream:**
  - **episode conditions:** a fact or a number computed from a column, at least / at most / lowest or highest N%;
  - **range conditions:** they narrow `kept` (progress rate, similarity, category, objects visible).
  - The rule views move from the measures into the Select.
- **A published view** is one row per episode with `kept` and the columns. Training and Data read `kept`.

## Build order

Everything is built in Studio and tested end to end on the 100-episode sample.

1. **Fix the contracts first:** the Select's new rule in the curation document, the training config and run request for the new family, the published model record, and the Score progress node's model field.
2. **Build in parallel:**
   - the Curation rule (step 3);
   - the training family, whose Modal job runs the upstream WARP-RM trainer (step 5);
   - model publishing, with the scorer loading published models (steps 6–7).
3. **Run the loop on the sample:** curate, publish, train, publish the model and score with it.
4. **Full import** (step 1, with whoever owns the LeRobot import), then run the loop again at full scale. The answers are stored, so takes replay fast.
5. **Record** with the tour's kit: shots, plain cuts and stored answers.

## Decisions for you

1. **Dataset scale.** Decided October 8: build and test the whole pipeline on the 100-episode sample, and import all 2,438 episodes later, before recording. On the sample, the pipeline is what's being tested, not the model: 13 minutes of experts probably won't find the stall. The sample has exactly 100 episodes, the preview's cap, so its preview and its published view see the same episodes. A "fastest 25%" that ranks only the preview's episodes would still pass, so its tests need more than 100 episodes.
2. **Where the bottle count comes from.** Decided October 8: Track objects, so no labels. Its count is off by one on 12% of the sample. The recorded counts (`meta/object_counts.json` in the source dataset) only check it, off screen.
3. **Curation shape.** Decided October 8: the [Curation refactor](#the-curation-refactor) below.
4. **Where models live.** I recommend a Studio record with the weights on the training Volume. Publishing to Hugging Face or into SpiralDB is more work and isn't needed for the loop.
5. **From scratch or fine-tuned.** I recommend from scratch, as in the paper, so the claim "trained on your experts" is plainly true. We compare it with the released head off screen.
6. **Length.** About 90 s, like the tour: the loop has more steps than a series video.
