# Series video 1: Score progress

Drafted September 26, 2026, against the current Studio. **Target: under 60 seconds, including the title card, end card and crossfades, so the footage is at most 57 s.** It follows the series pattern: start from a problem, curate the dataset in Spiral, publish the curation, and open the published curation in Data to check it. This video renews the September 23 demo and uses today's graph: Dataset → Select → Score progress → Publish → Data.

**This round is for story and pacing.** The proof-of-concept take was recorded with the headless Brave driver, so its cursor is a drawn overlay and its typing is synthetic: character by character through the DevTools protocol, not native keys. The serious take comes later, once the story is approved. That take uses native Safari capture with the real cursor and character typing, plus the zoom and highlight polish. See [Recording evidence](#recording-evidence).

Voice: Umbriel with the `pro-grounded` direction ([voice tests](../../../exports/demo-series-2026-09-26/voice-tests/README.md#customer-and-investor-direction-umbriel-video-1-september-26-second-round)).

## Narration and screen action

The times come from the current rough cut (`rough-cut-voiced.mp4`, **45.1 s**; the Remotion render with its cards is 47.2 s). It is assembled from shots: `shots.json` names the take chosen for each, and `edit_from_log.py` writes the edit from them (kit/README.md, "Shots and manifests"). Its layout:
- **intro:** the hero episode, then the cursor heads for Curation. Before the intro, off camera, the curation was started over `warp_sim_bottles_sample` and named **Long bottle demos**.
- **graph, score, inspect, publish:** one recording. Before it, off camera, the hero's episode tab was closed, so Data later opens on the dataset.
- **data:** the September 27 take's own data shot.
- The shots meet on still frames that match, pixel for pixel, joined by plain cuts. There are no dissolves and no errands on camera.

Pacing follows the [demo guide](../guide.md):
- **Primary moments play at 1×, with room to read:** the hero's turning bottle, the scored strip, the cutoff, the stretch it leaves out and the loaded view.
- **Secondary steps move fast:**
  - moves and dwells before secondary clicks run at 2×, and the Publish panel and the hop to Data at 3×;
  - other cursor moves run at 1.6×, rests at 10× and typing at 15×.
- **Model waits:** scoring (115 s) and the view read (119 s) become eased time-lapses of 1.2 s and 0.25 s.
- **Silences:** at most 1.14 s between lines, except 1.45 s before the publish line and 1.48 s before the Data line, where the picture can't shrink further without rushing.

The narration is Umbriel with the pro-grounded direction, 109 words. **Edit the narration and beats here.** The take and edit are re-pinned from this section.

**0:00–0:14: The problem, on a recording.** The hook comes first, so the video never opens on silence.
- **The hook.** It runs to 0:03.9. In the Remotion render it starts 0.5 s into the title card (a J-cut), 0.4 s before the picture. The rough cut holds its first frame for that 0.4 s instead.
- **The opening.** The cut opens on episode `…a0159504` (60.5 s), just opened from `warp_sim_bottles_sample` in Data. It plays at 1× under the hook, seek and Play included. Seek on the ruler at 0:02.0: the driver aims at 45.5 s and the playhead lands on 0:45.8. Play at 0:04.1.
- **The turning.** "Watch the left gripper" starts at 0:04.2. The stretch Score progress later leaves out, 46.5–49.2 s, plays at 0:04.8–0:07.5. The episode plays at 1× for 7 s from Play, then at 1.4× for half a second.
- **Into Curation.** The cursor heads for Curation at 0:12.0, under "We can easily remove moments like this with Spiral" (0:10.0–0:12.9). Curation opens at 0:12.8, already named and over the dataset, and "…with Spiral" ends there. The cut to the graph shot is at 0:13.8.

> Hesitations in your training data can leak into how your robot moves.

> Watch the left gripper. It keeps turning the bottle instead of dropping it in the bin.

> We can easily remove moments like this with Spiral.

**0:14–0:19: Filter.** Dataset + (0:14.6) → **Select** (0:15.5) → drag Duration to **at least 45.2 s** (0:17.2–0:18.3), which keeps 10 of 100. The line runs 0:13.4–0:19.2. It ends just after the drag lands, over "10 of 100": the bridge before it leaves no room to end on the drag itself.

> First, Spiral lets you quickly filter episodes by metadata, like duration or message count.

**0:19–0:26: Score progress.** The line starts at 0:19.9, as the menu under Select opens → **Score progress** (0:20.4) → the primer draws its progress-rate curve at 2.2× → **Score 10 episodes** (0:23.4). The 115 s wait eases into 1.2 s under "Spiral rates every moment…". The scored strip lands at 0:26.0, 0.4 s after the line ends: Kept 49 / Left out 40, keeping 7:49.1 of 8:17.0.

> Now let's score progress. Spiral rates every moment by how much it moves the task forward.

**0:26–0:29: How much is enough.** Drag the cutoff from 0.00× to **−0.10×** (0:27.9–0:29.0): Left out 40 → 30, keeping 7:53.6 of 8:17.0. The line runs 0:26.5–0:29.2 and ends as the drag lands.

> Then I choose how much progress is enough.

**0:30–0:35: The same moment, left out.** The line starts on **Left out** (0:29.9). "That same moment" lands as the hero's stretch **0:46.5 – 0:49.2** (2.7 s, −0.81×) is clicked at 0:31.3. It replays at 1× for 2.7 s, its whole length, with the cursor on its row, and the gripper is turning the bottle again. The hero's group reads "leaves out 0:04.3 of 1:00.5". Every left-out stretch is below the cutoff; nothing else is cut.

> Here's that same moment, left out. Only the moments that make no progress go.

**0:35–0:40: Publish.** Publish (0:35.4) → type the view name `long_demos_steady` first (0:36.4, at 15×) → tick Score progress (0:37.9) → Publish (0:38.8). The Published flag appears at 0:39.2, and "…to the Spiral catalog" ends 0.45 s after it.

> Now we can publish this curation to the Spiral catalog.

**0:40–0:45: Check it in Data.** Data (0:40.3) opens on the dataset's Overview, with no stale episode. The cut to the data shot is at 0:41.6, on the cursor resting on `long_demos_steady` → open it (0:41.8). The 119 s read eases into a quarter of a second. The view loads at 0:43.1: 40 intervals in 10 episodes, 7 min 54 s kept of 8 min 17 s. The line ends 0.4 s later, and the cut holds on the full app until 0:45.1, with no episode or slice played.

> Back in Data, the new view is ready for training.

## Paper citation overlay

Reuse the September 23 overlay on the hook ("Hesitations in your training data can leak into how your robot moves."): *Yu et al., WARP-RM (2026)*, https://arxiv.org/abs/2606.28320. It shows from the first frame of the picture until "Watch the left gripper". The claim is conditional ("can leak"), as in the earlier cut. The paper found that admitting longer, less efficient successful recordings degraded behavior cloning in its folding experiment. This video measures no training result.

## Verified numbers (September 27 take, current UI)

- **Dataset:** `dataset_warp_sim_bottles_sample`, 100 simulated episodes, in recording order by source path; every start time is 0. Keep internal identifiers out of narration. Present it as human-teleoperated simulation, not physical-robot footage.
- **Select:** Duration at least 45.2 s keeps **10 of 100** (Kept 10 / Left out 90). Any threshold from 45.0 to 45.5 s gives the same 10; the driver checks that the drag settles in that range. The strip's axis starts at 0, so the first drag doesn't rescale it. Kept episodes by catalog rank: #14 `…a0159504` (60.5 s), #17 `…3079b58f` (53.3 s), #19 `…09e1d65c` (47.8 s), #23 `…5f74e004` (49.4 s), #25 `…f8aef91b` (46.8 s), #27 `…6839e778` (46.2 s), #62 `…cf874f34` (45.5 s), #79 `…daf69d65` (51.5 s), #90 `…19f22d93` (45.8 s), #98 `…e4c8004b` (50.0 s).
  - The filter line names message count as an example of metadata. In this dataset it adds nothing as a second condition: every episode records exactly 120 messages per second (4 streams at 30 Hz), so the video uses duration alone.
  - A file-size cap would drop the hero.
- **Score progress:** 10 of 10 episodes scored. At the default cutoff (0.00×): Kept 49 / Left out 40, keeping 7:49.1 of 8:17.0. At **−0.10×**: Kept 40 / Left out 30, keeping 7:53.6 of 8:17.0.
- **Hero:** episode #14, catalog id `01a0b63c856c778e9702d903a0159504` (shown as `…a0159504`), 60.53 s, 7,268 messages. It is the same episode as the September 23 cut.
  - At −0.10×, its left-out stretches are 0:26.5–0:27.0 (−0.23×), 0:28.5–0:29.2 (−0.18×), 0:29.9–0:30.2 (−0.16×), **0:46.5–0:49.2 (2.7 s, −0.81×)** and 0:53.0–0:53.1 (−0.19×). Its group reads "leaves out 0:04.3 of 1:00.5".
  - At 0.00×, they were 26.5–27.0, 28.4–29.5, 29.7–30.3, 46.4–49.2 (2.8 s, −0.78×) and 53.0–53.2, plus one zero-length frame at 6.5.
  - The left camera shows the gripper turning the bottle from about 45.5 s until it carries it over the bin at about 52 s. The model flags it as low progress; that is not independently labeled proof of an error.
- **Published view** `long_demos_steady` (curation "Long bottle demos"): 40 intervals, 10 episodes, 7 min 54 s kept of 8 min 17 s, the same totals as the graph at −0.10×. The hero lists 6 kept intervals, and the gap between 0:30.2–0:46.5 and 0:49.2–0:53.0 is the stretch left out. A view reads the first 20 of the episodes the dataset's Selects keep, in recording order.

## Measured timings (headless Brave, 1440 × 900 at 2×)

| Step | Time |
|---|---|
| Choose dataset → episode index and overview | under 1.5 s |
| Add Select → workspace; first episode's cameras | instant; about 1.5 s |
| Add Score progress → primer animation | about 2.5 s |
| Score 10 episodes (local WARP, 8 in flight) | 115 s in the September 27 take; 85–126 s in earlier runs |
| Publish (saves, creates the view, records it) | 0.6 s |
| Data read of the 10-episode view | 119 s in the take; about 128 s in the practice take |
| Open an interval → the stretch playing | under 0.5 s |

## Fixes made for this video

- **A dense strip reads as a density.** With hundreds of scored moments, the cutoff strip's dots used to merge into a solid bar. A strip with 400 or more dots now draws each one faint, and left-out dots fainter still, so the cloud shows where moments bunch; a hovered or current dot still shows at full strength. `crates/frontend/styles/curation-threshold.css`. Before and after: `exports/demo-series-2026-09-26/01-score-progress/fixes/score-strip-before.png`, `score-strip-after.png`.
- **The Publish panel no longer asks for a name before you've had a chance to type one.** A ticked view with an empty name says "Name the view." only after its field is left empty or Publish is pressed; a typed name that can't be used is flagged as it is typed. `crates/frontend/src/activities/curation/graph/publish.rs` (`PublishChoice::shown_problem`), `crates/ui/src/controls/field.rs` (`TextInput` gained `onblur`). Stills: `exports/demo-series-2026-09-26/02-break-down-subtasks/fixes/publish-name-*.png`.

## Recording evidence

- **Shots:** `exports/demo-series-2026-09-26/01-score-progress/shots.json` names the take chosen for each shot. The recordings and their logs are in `shots/`.

  | Shot | Take | Recorded |
  |---|---|---|
  | intro | `shots/0927-184837-intro.mp4` | September 27, 18:48 |
  | graph, score, inspect, publish | `shots/0927-191220-graph+score+inspect+publish.mp4`, one recording | September 27, 19:12 |
  | data | `raw-take.mp4` from 208.07 s | the whole take of 10:45 |

  - **Joins:** `uv run ../kit/shots.py joins shots.json` diffs the last frame before each cut against the first frame after it.
    - intro → graph: 276 px differ (0.005 % of the frame, peak 20 of 255).
    - publish → data: 57 px differ (peak 13).
    - Both are encoder noise, with nothing moved. The shots in one recording aren't cut at all.
  - **Capture:** 2880 × 1800 lossless PNG frames from the 1440 × 900 page, tagged sRGB → BT.709 (kit/README.md, "Recording and colour"). Brave ran with `--window-size=1440,1037 --force-device-scale-factor=2`, and the driver defaults to `--dpr=2`. The screencast delivered 34.5 frames per second over the graph-to-publish recording, into a 30 fps file.
  - **Driver:** `take-01.mjs` lists the six shots for `../kit/shots.mjs`. It uses the kit's `HumanInput`, which gives every shot its own seed: curved, eased cursor paths, a dwell before every press, and uneven key-by-key typing. The cursor is a drawn overlay and typing goes through the DevTools protocol; neither is native input.
    - `node take-01.mjs --shot=graph` records one shot. The shots before it replay off camera first, as fast as the app allows.
    - Score, inspect and publish are one group, recorded together: a scored episode plays in the preview as soon as scoring lands, so the joins between them can't be rebuilt off camera.
  - **Errands, off camera:**
    - Starting the curation over the dataset and naming it: the intro's setup. Choosing a dataset names the curation after the dataset, so the name goes in second.
    - Closing the hero's episode tab: the graph's setup, with the cursor put back on the Curation tab.
    - The Published toast clears before the publish shot's last frame.
  - **Logs:** `shots-input.log` is every chosen shot's log on the edit's clock. It has every move, click, drag, key, wait and mark, plus what the app answered: the Select's count, the scored and cutoff counts, the hero's row and group, and the view's facts and hero rows.
  - **Earlier takes:** the whole take of September 27, 10:45 (`raw-take.mp4`, `take-input.log`) still provides the data shot, and was the first take of every shot (sliced at the dwell before each shot's first press). Its driver is kept as `take-01-whole.mjs`. The September 26 take is in `take-1/`, and a practice take is in `take-2-practice/`. The graph-and-score session deletes the previous "Long bottle demos" first: publishing a view name that already exists replaces the view, and deleting a curation drops its views by name.
  - **Dry run:** `node take-01.mjs --dry` drives the shots without recording, under its own curation name ("Long bottle demos dry run") and view name (`long_demos_dry_run`). Delete its curation afterwards, which drops its view.
- **Rough cut:** `rough-cut-voiced.mp4`, **45.1 s** at 2880 × 1800 and 30 fps, tagged BT.709 limited range with an sRGB transfer. `edit_from_log.py` writes `edit.json` (and `shots-input.log`) from `shots.json`, and `../kit/edit.py render edit.json` renders it. The pacing it shares with video 2 lives in `../kit/take_plan.py`. The beat times are in [Narration and screen action](#narration-and-screen-action).
  - **Joins:** plain cuts at 0:13.8 and 0:41.6, each on a still frame that matches. There are no dissolves.
  - **Zones:**
    - The opening plays at 1× up to Play, under the hook. The hero plays at 1× from Play through the turning (7 s), then at 1.4× until the cursor heads for Curation.
    - Secondary clicks run their move and dwell at 2×, and the Publish panel, Data and the view at 3×. The press stays at 1×. A long wait between arriving and pressing runs as a rest.
    - Other cursor moves run at 1.6×. Rests and settles of 0.4 s or more after the intro run at 10×.
    - Typing runs at 15×. The duration drag runs at about 1.3× and the cutoff drag at about 1×; the primer runs at 2.2×. Data's page switch runs at 2×.
    - Scoring (115 s) eases into 1.2 s, and the view read (119 s) into 0.25 s.
    - At 1×: the turning (7 s), the scored strip (0.8 s), the stretch's replay (2.7 s), the Published flag (0.3 s), and the loaded view (2.0 s).
  - **Voice:** Umbriel pro-grounded. Each line is cut 0.08 s either side of its speech and pinned to the action it describes. The hook is placed at −0.4 s, before the picture. For each separate render I made three takes and picked the one nearest its neighbours' pitch, with no ticks and the script exact.

    | Line | File | Pitch |
    |---|---|---|
    | the hook | `series01-v4-hook-a.wav` | |
    | "Watch the left gripper…" | `series01-v4-watch-a.wav` | 94.9 Hz |
    | the bridge | `series01-v5-bridge-c.wav` | 102.6 Hz, between the watch and filter lines |
    | the filter line | `series-filter-a.wav` (the same render video 2 uses) | 112.7 Hz |
    | the score line | `series01-v6-score-c.wav` | 113.2 Hz |
    | the "enough" line | `series01-v6-enough-a.wav` | 114.8 Hz |
    | the moment line | `series01-v6-moment-c.wav` | 117.4 Hz; take b had two ticks |
    | the publish line | `series01-v3a.wav`, the full take | |
    | the Data line | `series01-v3a.wav`, the full take | |

- **Remotion render:** `remotion/out/video-1.mp4`, 47.2 s with the title and end cards.
  - **Camera:** it zooms only twice: on the hero's cameras at the opening, and on the left-out stretch replaying. Everything else is a spotlight at the full frame, and it ends on the full app.
  - **Checks:** colour tags sRGB transfer, BT.709, limited range; loudness −16.1 LUFS integrated, true peak −4.6 dBTP; on-device speech recognition heard all 109 words in order; captions a median 45 ms from the heard word.

## Before recording the native take

- Start Curation from a fresh page so it opens on a blank graph, and start Data with nothing open.
- Episodes remember their layout per dataset in `localStorage`. If a layout was dragged by hand, reset it so the designed one shows (three cameras one over two beside the Scene).
- The Curated rail also lists `placing_skill` (video 2) and the older `similar_task_view`, marked Changed. The user decides whether they stay for the recording.
- Score progress takes about 2 min and the view read about as long. The edit eases both into time-lapses.
