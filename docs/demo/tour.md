# Tour: Spiral Robotics, built on SpiralDB

Drafted October 7, 2026. **Target: about 90 seconds, including the title and end cards.** Audience: SpiralDB customers. The tour shows what you can build on SpiralDB infra. Spiral Robotics is the example: you browse a robot dataset, curate it with models, and get training data out.

It follows the [demo guide](guide.md) and reuses beats verified in the [series](series/). The open in Data and the hero's turning bottle come from [video 1](series/01-score-progress.md), and Break down with Find similar from [video 2](series/02-break-down-subtasks.md). The new part is that both curations are stacked into one graph:

```text
Dataset → Select (10 longest) → Score progress → Break down subtasks → Find similar → Publish → Data
```

**First take: October 7, 2026.** The rough cut is `exports/demo-series-2026-09-26/tour/rough-cut-voiced.mp4`: 90.5 s, opening with a spoken greeting and with no end card yet. The second round (same day) added the opening and reworded the lines to sound more like a person talking. It was recorded in headless Brave with the series kit, against the dev server on :3000. Every number below is from that take. [First take](#first-take) has the evidence and what's still rough.

## The story

You open a demo in Data. Every camera and joint plays from SpiralDB, and you can scrub through it. One demo has a problem: the left gripper keeps turning a bottle instead of dropping it. In Curation, a filter, a progress model, a step-by-step breakdown and a search turn the dataset into every drop with the moments that make no progress left out. You publish that as one view and open it in Data, ready for training.

Every infra line names something SpiralDB does on screen: the recordings are stored in it, an episode opens with a query, each node compiles to SQL, model answers are saved back into it, and the curation becomes one SQL view.

## Narration and screen action

The times are from the round-2 rough cut (90.5 s). The polished render tightened a few beats after 0:28, so from there its times run up to 3 s earlier. The spoken opening replaces the title card.

The narration is 217 words, cut from one full Umbriel pro-grounded take (`voice-tests/norm/tour-v2c.wav`, the best of three: 168.5 wpm, 112.7 Hz). The take has one tick, before "The scores…", and that line's clip starts just after it. On-device speech recognition hears every word of the mix in order. **Edit the narration and beats here.**

Pacing follows the [demo guide](guide.md):
- **At 1×:** the opening in Data, the turning, the scored strip (2.4 s), the replayed stretch (2.7 s), the storyboard (0.6 s), the first results screen (3 s) and the loaded view (4 s).
- **Faster:** secondary clicks at 2×, routine ones from Break down on at 3×, moves at 1.6×, rests at 10×, and typing at 5× for the query and 15× for the view name.
- **Model waits:** the answers are stored, so the waits are short. Scoring (about 3 s) eases into 1.6 s, Break down (2 s) into 0.6 s, and the search runs at 5×.

**0:00–0:09: Opening.** The cut opens on Data's datasets. The cursor rests, then glides down the rows (abc130k_sample, default) to `warp_sim_bottles_sample`, and opens it as the line ends.

> Hi there! Let me show you what you can build on SpiralDB. This is Spiral Robotics. It turns robot recordings into training data.

**0:09–0:37: Data.**
- **The dataset.** The overview shows 100 episodes, 50 min 34 s and 1.7 GiB, with the dataset checks. It holds 6.5 s for line 1.
- **The hero.** The rail wheels down to episode `…19475a6a` (1:00.5), opened at 0:17.5. It shows three cameras (top, left, right), the `/lerobot/frame` joint plots and the Timeline. This dataset has no robot model, so there's no Scene.
- **The scrub.** At 0:20 the cursor drags the playhead from 0:00 to 0:45.8 at 1× over 2.8 s, and the cameras and plots follow it. It holds there for line 3's first sentence.
- **The turning.** Play at 0:27.5. The stretch Score progress later leaves out (0:46.5–0:49.2) plays at 0:28–0:31. After the turning, playback runs on at 1.4× until the cursor heads for Curation, which opens at 0:36.7, already named "Steady drops" and over the dataset.

> This dataset has a hundred demos. Each one has three cameras and every joint, recorded thirty times a second, and it's all stored in SpiralDB.

> Opening one is just a query. I can scrub anywhere, and the cameras and joints follow along.

> But not every second is worth training on. Watch the left gripper. It keeps turning the bottle instead of dropping it in the bin.

> So let's find all the drops, and leave moments like this out.

**0:37–0:43: Filter.** Dataset + → **Select** → drag Duration to **at least 45.2 s**, landing at 0:42. That keeps 10 of 100, and "…ten longest demos" ends 1.1 s later.

> Every step in this graph becomes SQL that SpiralDB runs. First, I keep the ten longest demos.

**0:43–1:00: Score progress.**
- Select + → **Score progress** at 0:44. The primer plays at 1×, then **Score 10 episodes** at 0:47, which reads the stored scores. The strip lands with Kept 57 / Left out 48 at 0.00×.
- At 0:53 the cutoff sweeps up to 0.50× and back down to **−0.05×**. That gives Kept 48 / Left out 39, keeping 7:51.5 of 8:17.0. (At −0.10×, today's scores split the hero's stretch in two.)
- **Left out** → the hero's stretch **0:46.5 – 0:49.2** (2.7 s, −0.73×) is clicked at 0:57. It replays whole at 1×. Its group reads "leaves out 0:04.5 of 1:00.5".

> Score progress rates how much each moment moves the task forward. The scores are saved back to SpiralDB, so nothing gets scored twice.

> I choose how much progress is enough, and there's that moment from before, left out.

**1:00–1:08: Break down.**
- Score progress + → **Break down subtasks** at 1:01 → the primer at 3× → **Break down 10 episodes**. The rows land at 1:05: 10 of 10 episodes, 163 subtasks.
- The hero's "Place the bottle in the bin with the right arm" (0:04.0 – 0:12.0) is clicked, and the Subtasks lane holds.
- The stacked beat is on screen: the hero's "Hand the bottle between both arms" stops at 0:46.5 and picks up at 0:49.2, around the stretch Score progress left out.

> Then Break down cuts what's left into small steps, and describes each one.

**1:08–1:19: Find the drops.** Break down + at 1:09 → **Find similar subtasks** → type "Put an object into a container" at 5× (1:12) → Search. At 1:15 the results land: "Found in 10 of 10 episodes (best match ≥ 50%)", 53 of 163 subtasks. The top match is "Drop the cola bottle into the green bin with the right arm" (…8df6df68, 94%). The first results screen holds 3 s.

> Now I search for the step I want: putting an object into a container.

> And it turns up in all ten demos.

**1:19–1:24: Publish.** Publish at 1:19 → tick Find similar → name the view `steady_drops` at 15× → Publish. The Published flag appears at about 1:22.5, as the line ends.

> When I publish, the whole graph becomes one SQL view.

**1:24–1:30.5: Check it in Data.** Data → `steady_drops` in the Curated rail. The view loads at once at 1:26.5: 53 intervals, 10 episodes, 2 min 44 s kept of 8 min 17 s. It shows the same descriptions and similarities as Curation, because a view reads the stored answers. The cut holds on the full app to the end, with no slice played. Score progress leaves out only the stretch below the cutoff, so a hero slice can still show some turning.

> Back in Data, we've got every drop with the stalls cut out, ready to train on.

**End card** (not in the rough cut yet): the wordmark and spiraldb.com.

## What each infra line rests on

| Line | What's true underneath | Source |
| --- | --- | --- |
| "it's all stored in SpiralDB" | `warp_sim_bottles_sample` is the Spiral tables under `datasets.warp_sim_bottles_sample`, written when its MCAP files were imported. Every topic, the camera video included, is a table of its messages. | [datasets.md](../datasets.md#catalog-layout) |
| "Opening one is just a query" | Data reads every camera and joint stream with SQL over its `messages` table, bound to the episode. There are no video or joint endpoints. | [episode-playback.md](../episode-playback.md#one-message-transport) |
| "Every step in this graph becomes SQL that SpiralDB runs" | `CurationPath` compiles each node's step. A preview runs the node's step, and the published view chains every step into one statement. | [curation.md](../curation.md#compiling-a-path) |
| "The scores are saved back to SpiralDB, so nothing gets scored twice" | Each model call is a column of `datasets.<id>.answers`, a Spiral table. A preview fills its column first, then reads it, and identical calls share a column. | [curation.md](../curation.md#stored-answers) |
| "the whole graph becomes one SQL view" | A published view is one read-only statement over every episode. Studio stores it, not the Spiral catalog. **The series line "publish this curation to the Spiral catalog" is no longer true; don't reuse it.** | [curation.md](../curation.md#publishing) |

What the narration avoids claiming:
- that the models run inside SpiralDB (WARP scoring runs on our Modal service, and Studio's backend asks OpenRouter for Break down and Find similar);
- that the demos are physical-robot footage (they're human-teleoperated simulation);
- that every fumble is gone. Only the moments below the cutoff are, which is why the last line says "the stalls".

## First take

- **Footage:**
  - **Files:** `exports/demo-series-2026-09-26/tour/`. `take-tour.mjs` drives eight shots with the kit's `ShotRecorder` and `HumanInput`, and `shots.json` names the takes.
  - **Recordings:** `shots/1008-*-intro.mp4` and `shots/1008-*-graph+…+data.mp4`, as `shots.json` names them. Both are 2880 × 1800 at 27–30 distinct frames a second.
  - **Join:** one plain cut, where the two files meet. It differs by 262 px (encoder noise).
  - **Run time:** a whole run takes 150 s, because every answer is stored: Score progress, Break down and the search each take seconds.
- **Edit:** `edit_from_log.py` writes `edit.json`, and `../kit/edit.py render edit.json` renders `rough-cut-voiced.mp4` in about 30 s.
- **Browser:** Brave runs headless on port 9333 with `tour/brave-profile`, which is signed in to Studio on :3000. `window-size.mjs` sets its view to 1440 × 900, because the profile remembers a window size and ignores `--window-size`.
- **What changed since the draft:**
  - The dataset was rebuilt, so its episode IDs changed. The hero is now `…19475a6a`.
  - The dataset has no robot model, so line 2 names cameras and joints instead of the robot.
  - The cutoff is −0.05×, not −0.10×.
  - Curation reopens the curation last open. The kit's `StudioPage.startCuration` now starts a new one from the switcher, and it targets :3000, the only server that can sign in.
- **Still rough:**
  - Gaps of 2.5–3 s between some lines in Curation, while Break down, the search and Publish are clicked through.
  - Camera tiles show "Loading…" for a moment just after scoring and after the search.
  - The "Publishing steady_drops" toast is still visible as Data opens.
- **Polished render:** `exports/demo-series-2026-09-26/remotion/out/tour.mp4`, 89.1 s at 1920 × 1080 (spec: `remotion/videos/tour.json`). It's the rough cut plus captions, one zoom, eleven spotlights and a 2.0 s end card (wordmark, "Built on SpiralDB", spiraldb.com). The spoken opening replaces the title card.
  - **Under 90 s:** the cut now runs 87.5 s. The hero plays at 1× only through the turning, the query types at 8×, the results hold 2.3 s and the loaded view 3 s.
  - **Zoom:** one, at 1.25×, on the hero's cameras while the gripper turns the bottle. Every other beat is a spotlight on the full app.
  - **Spotlights, one at a time:** the dataset's facts, the left camera, the Duration condition and then Kept 10, the cutoff's sweep, the hero's left-out row and then its replay, the storyboard lane, "Found in 10 of 10 episodes", the Published flag, and the view's facts.
