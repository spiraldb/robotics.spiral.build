# Demo: Your own progress model

Drafted October 8, 2026. **Status: script draft.** Nothing is recorded yet. **Target: about 90 seconds, at most 120, including the end card** (the current draft runs near 105). Audience: customers and investors, as for the [tour](tour.md).

It extends [video 1](series/01-score-progress.md), and the motivation is the same: hesitations in the demos leak into how the robot moves. In video 1 a released progress model found them. Here the viewer trains their own progress model, on their own best demos, and the demo goes all the way to training the robot's policy. It is the paper's method (WARP-RM, then a policy trained on the demos it cleans), done in Studio:

```text
1 Pick the experts     Dataset → Track objects ("bottle") → Select (duration per bottle, lowest 25%) → Publish expert_demos
2 Train a progress model   Training → Progress model on expert_demos → Publish "Bottle experts"
3 Clean every demo     Dataset → Score progress (Bottle experts) → Select (progress rate at least the cutoff) → Publish clean_demos
4 Train the policy     Training → ACT policy on clean_demos
```

It follows the [demo guide](guide.md), with two departures, both because the story is longer:
- **Chapters.** A chapter bar is on screen from chapter 1 on. See [The chapter bar](#the-chapter-bar).
- **The ending.** The last shot is the policy training on the clean demos, not Data. We can't show a better robot in 90 seconds, so the proof is the paper's measured result, shown as a citation like video 1's.

The build plan and what's still missing are in [train-on-experts.md](train-on-experts.md).

## The chapter bar

A thin bar along the top of the frame, outside the app, with four labelled segments:

```text
 1 Pick the experts │ 2 Train a progress model │ 3 Clean every demo │ 4 Train the policy
```

- It appears with the plan line, all four chapters dim, and stays until the end card; chapter 1 lights as the first curation starts.
- The current chapter's label is full strength and its segment fills left to right as the chapter plays; finished chapters stay filled, later ones dim. Lime for the fill, as the subtitle highlight uses it.
- It never moves and never animates beyond the fill, so it costs no attention. It's built in the Remotion edit, not in the app.

## Narration and screen action (draft)

The structure and opening are the user's (October 8); the lines are worded to the [demo guide](guide.md). About 225 words: roughly 80 seconds of speech at the series' pace, so with the time-lapses and the beats at 1× the cut lands near 105 seconds. To get closer to 90, the paper line and the plan line are the first to shorten. Times are targets, not measured.

**0:00–0:16 · Opening (Data).** The full dataset in Data (about 2,400 episodes on screen), then the hero episode. Its turning bottle plays at 1×, as in video 1.

> Here are about 2,400 successful demos of a robot clearing bottles into a bin. But is every moment in them worth training on?

> Look at this one. The left gripper keeps turning the bottle instead of dropping it.

> Moments like this in your training data can leak into how your robot moves. Spiral's curation can take them out.

**0:16–0:24 · The plan (Curation opens).** The chapter bar comes up during this line, all four chapters dim, so the plan and the bar arrive together.

> First, we find the most successful demos and train a progress model on them. Then we use that model to find moments like this in every demo.

**0:24–0:42 · 1 Pick the experts (Curation).** Dataset → **Track objects**, "bottle" on the top camera. The fill is a time-lapse; an episode opens with every bottle boxed. → **Select**: Duration per bottle, lowest 25%. The strip lands on the cut. → Publish `expert_demos`, fast.

> Successful here means fast: the shorter, the better.

> Track objects finds the bottles in every demo. Then I keep the top quarter with the shortest time per bottle, and publish them.

**0:42–0:58 · 2 Train a progress model (Training).** Progress model → View `expert_demos` → Run training. The run is a time-lapse of its loss curve. On the finished run, publish the best checkpoint as "Bottle experts".

> Now in Training, I train a progress model on those demos, on a cloud GPU, with the video streamed from SpiralDB. Then I publish the model.

**0:58–1:24 · 3 Clean every demo (Curation).** A new curation over the full dataset → **Score progress**, Score with "Bottle experts". The scoring is a time-lapse. → **Select**: Progress rate at least the cutoff. Left out → the hero's stretch replays at 1×, the bottle turning. → Publish `clean_demos`, fast.

> With our model, I make a new curation over the full dataset. It scores every moment of every demo.

> I keep the moments that make progress, and there's that turning bottle, left out. Then I publish the clean demos.

**1:24–1:42 · 4 Train the policy (Training).** ACT policy → View `clean_demos` → Run training. The run is a time-lapse of its first minutes, the loss falling. The paper citation overlay comes up with the last line.

> Last, I train the robot's policy on the clean demos.

> In the WARP-RM paper, a policy trained on progress-filtered demos cleared all six bottles in a quarter of its runs, against one in ten for the raw demos.

**1:42–1:45 · End card.** Wordmark and spiraldb.com.

Two wording choices, kept true to the screen:
- **"Track objects finds the bottles"**, not "object detection": the node on screen is Track objects (SAM 3.1 detects and then follows each bottle).
- **"progress model"**, not "curation model": that is the name in Training's model choice and the model Score progress uses.

Pacing, by the [demo guide](guide.md):
- **At 1×:** the turning bottle in the opening, the strip landing on the expert cut, the trained model's loss curve settling, the hero's stretch replaying in chapter 3, and the policy's loss falling.
- **Fast:** dataset choice, node menus, publishing, the Training form.
- **Time-lapses:** Track objects (hours over the full dataset), the progress model's training (about half an hour), the scoring with our model, and the policy's run. Every answer and checkpoint is stored, so a take only waits for them once.

## What each claim rests on

| Line | What's true underneath |
| --- | --- |
| "about 2,400 demos, each with two to six bottles" | The full import of `uynitsuj/sim-bottles-mjwarp-v1` has 2,438 episodes; the source's recorded counts are 2 to 6 per episode. Data shows the count on screen. |
| "Track objects counts the bottles" | SAM 3.1's most bottles seen at once. It matched the recorded count on 88 of 100 sample episodes and was off by one on the rest. |
| "the fastest quarter by time per bottle" | Select: Duration per bottle, lowest 25%, a percentile rank over every episode. The paper used the shortest 25% within each bottle count; ours is the same idea with one rule, and the narration doesn't claim it's the paper's. |
| "on a cloud GPU, with the video streamed from SpiralDB" | Studio reads the camera packets from SpiralDB and uploads them to the Modal job, which trains on an H100. |
| "our model scores every demo" | Score progress with the published model, over every episode reaching it. The answers are stored per model, so the released model's scores never mix in. |
| "there's that turning bottle, left out" | **Must be verified** on the trained model before recording: the hero's stretch (0:46.5–0:49.2 in the sample) must fall below the cutoff. If it doesn't, pick a hero this model does flag, and re-cut the hook to match. |
| the paper line | WARP-RM paper, simulated bottles: WARP-BC cleared all six bottles in 25.0% of 512 scenes against 9.4% for vanilla BC (the authors' `docs/reproduce_sim.md`). Their policy is pi0 with chunk gating; ours is ACT on kept intervals, so the line says "in the paper", shown with the citation on screen. |

What the narration avoids claiming:
- that our policy is better: we show it training, not its rollouts;
- that the experts are labelled by hand, or that the rule is the paper's exact one;
- that the models run inside SpiralDB (they run on our Modal services).

## Before recording

1. **Full dataset in Studio.** Spiral v0.16.45 (with the LeRobot producer and the release-build fix), Studio's driver bumped to it, Studio reading LeRobot datasets (topic listing), then `CREATE DATASET … USING spql.lerobot.dataset` and a refresh. See [train-on-experts.md](train-on-experts.md#how-far-we-are).
2. **Keep the best checkpoint.** On the 100-episode sample the progress model overfit after about 750 steps, and only every 2,500th step was saved. Training should keep the checkpoint with the lowest validation loss, and chapter 2 publishes that one.
3. **Verify chapter 3's beat** on the trained model: score the long demos and check the hero's stretch is left out at a cutoff that doesn't leave out most of the demo.
4. **ACT on a progress-cleaned view.** A cleaned view has many short kept ranges per episode. Check the ACT run admits it, how many samples it gets, and that its loss falls in the first minutes.
5. **Fill everything once, off camera:** Track objects over every episode, the expert view, the progress model's training, its scores over every episode, and the policy run, so each take replays from stored answers.
6. **The chapter bar** in the Remotion kit, with the other overlays.
