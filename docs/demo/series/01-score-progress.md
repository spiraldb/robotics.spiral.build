# Series video 1: Score progress

Drafted September 26, 2026, against the current Studio. **Target: about 60 seconds.** It follows the series pattern: start from a problem, curate the dataset in Spiral, publish the curation, and open the published curation in Data to check it. This video renews [the September 23 demo](../unproductive-moment-demo.md) and uses today's graph: Dataset → Select → Score progress → Publish → Data.

**This round is for story and pacing.** The proof-of-concept take was recorded with the headless Brave driver, so its cursor is a drawn overlay and its typing is synthetic: character by character through the DevTools protocol, not native keys. The serious take comes later, once the story is approved. That take uses native Safari capture with the real cursor and character typing, plus the zoom and highlight polish. See [Recording evidence](#recording-evidence).

Voice: Umbriel with the "founder" direction ([voice tests](../../../exports/demo-series-2026-09-26/voice-tests/README.md)). Umbriel leaves almost no pause before an emphasized word, so add about 120 ms in the edit where a sentence turns.

## Narration and screen action

Narration runs about 124 words, one sentence per line in [narration.txt](../../../exports/demo-series-2026-09-26/01-score-progress/voiceover/narration.txt). The Umbriel clips total 53.4 s. Beat times are the rough cut's (63.5 s; the final edit tightens it to about 60).

**0:00–0:11: The problem, on the recording.** Data, episode `…a0159504`. Seek to 45.5 s and play through the bottle turning at 46–49 s. All three cameras and the Scene are visible.

> Even successful robot demonstrations have slow moments: hesitations, repeated adjustments, regrasps. Train on them, and your policy can learn to hesitate too.

**0:11–0:16: Start an empty curation.** Open Curation, which starts on a blank graph, and choose `warp_sim_bottles_sample` from the dataset ledger. Rename the curation **Long bottle demos**, so the Data header reads well later.

> Let's curate this dataset in Spiral.

**0:16–0:25: Basic filtering.** Use the Dataset node's + to add **Select**. Slide Duration to **at least 45 s** (the take landed on 45.2 s; aim for 45.0 in the native take, and use the arrow keys if needed). The strip, counts and list go from 100 episodes to **Kept 10 / Left out 90**. The open episode moves to the first kept one.

> First, some basic filtering: keep only the longer episodes, forty-five seconds or more. That leaves ten of a hundred.

**0:25–0:35: Score progress.** Use the Select's + to add **Score progress**. The primer draws its progress-rate curve for about 2.5 s. Click **Score 10 episodes**, cut the scoring wait to its first and last seconds, then show the scored strip. Drag the cutoff from 0.00× to **−0.10×** (Left out 40 → 30). This is the "you decide how strict" beat.

> Then, Score progress. It rates how fast every moment moves the task forward, against the expert rate, and I choose how strict to be.

**0:35–0:44: The stretch it left out.** Click **Left out**, then the hero's stretch **0:46.5 – 0:49.2** (2.7 s, mean −0.81×). It plays from 46.5 s: the left gripper keeps rotating and readjusting the bottle before it moves on.

> Here's a stretch it left out: the gripper keeps turning the bottle instead of placing it. Only this stretch is dropped; the rest of the episode still trains.

**0:44–0:51: Publish.** Open Publish, choose Score progress, name the view `long_demos_steady`, and press Publish. Publish saves the curation itself. Cut the wait down to "Publishing as long_demos_steady", then the Published flag.

> Publish, and the curation becomes a view in the Spiral catalog.

**0:51–1:03: Check it in Data.** Open Data. The view is in the rail's Curated section. Cut the read down to its first second and the loaded view: **Episodes 10, kept time 7 min 54 s of 8 min 17 s**, the same as the graph's "Keeps 7:53.6 of 8:17.0", with the kept intervals under each episode. Open the hero's interval that ends at 0:46.5 and seek to 44 s. The playhead then crosses the gap the view leaves at 46.5–49.2 on the `long_demos_steady` row.

> Open it in Data: the same ten episodes, without the stretches that stalled, ready for training.

## Paper citation overlay

Reuse the September 23 overlay on "Train on them…": *Yu et al., WARP-RM (2026)*, https://arxiv.org/abs/2606.28320. The claim is conditional ("can learn"), as in the earlier cut. The paper found that admitting longer, less efficient successful recordings degraded behavior cloning in its folding experiment. This video measures no training result.

## Verified numbers (September 26, current UI)

- **Dataset:** `dataset_warp_sim_bottles_sample`, 100 simulated episodes, in recording order by source path; every start time is 0. Keep internal identifiers out of narration. Present it as human-teleoperated simulation, not physical-robot footage.
- **Select:** Duration at least 45 s keeps **10 of 100**. Any threshold from 45.0 to 45.5 s gives the same 10. The strip's axis now starts at 0, so the first drag doesn't rescale it. Kept episodes by catalog rank: #14 `…a0159504` (60.5 s), #17 `…3079b58f` (53.3 s), #19 `…09e1d65c` (47.8 s), #23 `…5f74e004` (49.4 s), #25 `…f8aef91b` (46.8 s), #27 `…6839e778` (46.2 s), #62 `…cf874f34` (45.5 s), #79 `…daf69d65` (51.5 s), #90 `…19f22d93` (45.8 s), #98 `…e4c8004b` (50.0 s).
  - Message count adds nothing as a second condition: every episode records exactly 120 messages per second (4 streams at 30 Hz).
  - A file-size cap would drop the hero, so the video uses a single condition.
- **Hero:** episode #14, catalog id `01a0b63c856c778e9702d903a0159504` (shown as `…a0159504`), 60.53 s, 7,268 messages. It is the same episode as the September 23 cut.
  - At the default cutoff (0.00×), its left-out stretches are 26.5–27.0, 28.4–29.5, 29.7–30.3, **46.4–49.2 (2.8 s, −0.78×)** and 53.0–53.2, plus one zero-length frame at 6.5.
  - At −0.10×, they are 26.5–27.0, 28.5–29.2, 29.9–30.2, **46.5–49.2 (2.7 s, −0.81×)** and 53.0–53.1.
  - The footage shows repeated re-orientation of the bottle in the left gripper. The model flags it as low progress; that is not independently labeled proof of an error.
- **Left out at 0.00×:** 40 stretches. Several last only one frame, the first reads `0:06.5 – 0:06.5 · 0.0 s · −0.00×`, and switching to Left out plays it. At −0.10× there are 30, and the first is the hero's 26.5–27.0.
- **Published view:** a view now reads the first 20 of the episodes the dataset's Selects keep, in recording order, not the dataset's first 20. The graph and Data agree: 10 kept, 10 in the view. `EXPLAIN` shows the plan runs `spql.ai.score_progress` above `TopK limit=20` above the duration filter, so only the selected episodes are scored.

## Measured timings (headless Brave, 1440 × 900)

| Step | Time |
|---|---|
| Choose dataset → episode index and overview | under 1.5 s |
| Add Select → workspace; first episode's cameras | instant; about 1.5 s |
| Add Score progress → primer animation | about 2.5 s |
| Score 10 episodes (local WARP, 8 in flight) | all 10 in 105 s (walkthrough) and 126 s (take); the first result took 9 s and about 25 s |
| Publish (saves, creates the view, records it) | 0.8 s in the take; 16 s in the first walkthrough |
| Data read of a 3-episode view (old bound) | about 32 s |
| Data read of the 10-episode view (new bound) | 119 s |
| Open an interval → episode tab playing | 1–2 s |

## Recording evidence

- Proof-of-concept take: `exports/demo-series-2026-09-26/01-score-progress/raw-take.mp4` (5 min 56 s), driven by `take-01.mjs` through the Chrome DevTools Protocol in headless Brave at 1440 × 900 CSS pixels. It is recorded at 1920 × 1200 by piping screencast frames to ffmpeg as 30 fps. `take-input.log` records every click, drag, type and wait against the recording clock, including the Select threshold, the cutoff, and the rows it clicked.
- The cursor is a drawn overlay that follows the synthetic pointer, and typing goes in one character at a time. Neither is native input. **The native Safari take**, with the real cursor, character typing, zooms and highlights, comes after the story is approved. Reuse the September 23 capture approach (`screencapture -v -C -l<window>`, CGEvent input), which needs Accessibility and Screen Recording granted to the app that drives it.
- The rough cuts use plain cuts only, from `edit.json` via `edit_take.py`: `rough-cut-silent.mp4` (63.5 s), and `rough-cut-voiced.mp4` with the Umbriel sentences placed at their beats. `rough-cut-contact.png` shows a frame every 3–4 s. Waits are cut rather than sped up; only the rename, the Publish panel and the view-name typing run faster (1.5–2×), along with the primer (1.25×). A 120 ms lead sits before the two sentences that turn ("It rates…", "Only this stretch…").

## Before recording the native take

- Start Curation from a fresh page so it opens on a blank graph, and start Data with nothing open.
- Episodes remember their layout per dataset in `localStorage`. If a layout was dragged by hand, reset it so the designed one shows (three cameras one over two beside the Scene).
- The Curated rail also lists the older `similar_task_view`, marked Changed. The user decides whether it stays for the recording.
- Score progress takes about 105 s and the view read about as long. Both are cut in the edit, not sped up.
