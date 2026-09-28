# Series video 2: Break down subtasks

Drafted September 26, 2026, against the current Studio. **Target: about 60 seconds.** It follows the series pattern: start from a problem, curate the dataset in Spiral, publish the curation, and open the published curation in Data to check it. The graph is Dataset → Select → Break down subtasks → Find similar subtasks → Publish → Data.

**This round is for story and pacing.** The proof-of-concept take was recorded with the headless Brave driver, so its cursor is a drawn overlay and its typing is synthetic. The native Safari take comes once the story is approved (see [video 1](01-score-progress.md#recording-evidence)).

Voice: Umbriel with the `pro-grounded` direction ([voice tests](../../../exports/demo-series-2026-09-26/voice-tests/README.md#customer-and-investor-direction-umbriel-video-1-september-26-second-round)). Most of the narration is cut at its pauses from one full take (`voice-tests/norm/series02-v3b.wav`), so the read stays one continuous performance; the lines that changed later are separate renders matched to its pitch (see [Recording evidence](#recording-evidence)).

## The story

Episodes are whole-task demonstrations: a person teleoperating the simulated robot drops every object into the bin. To train or evaluate one skill, such as placing an object in the bin, you need that skill's boundaries in every episode, and labeling them by hand is slow and inconsistent. Spiral breaks every episode into timestamped subtasks, finds the same skill across the episodes, and publishes just those slices as a view that Data can open and play.

## Narration and screen action

The times come from the current rough cut (`rough-cut-voiced.mp4`, **56.9 s**; the Remotion render with its cards is 59.05 s). It is assembled from shots: `shots.json` names the take chosen for each, and `edit_from_log.py` writes the edit from them (kit/README.md, "Shots and manifests"). Its layout:
- **intro:** the hero episode, whole, then the cursor heads for Curation. Before the intro, off camera, the curation was started over `warp_sim_bottles_sample` and named **Placing skill**.
- **graph, breakdown, search, publish:** one recording, the first of two variants. Before it, off camera, the hero's episode tab was closed, so Data later opens on the dataset.
- **data:** its own recording.
- The shots meet on still frames that match, pixel for pixel, joined by plain cuts. There are no dissolves and no errands on camera.

Pacing follows the [demo guide](../guide.md):
- **Primary moments play at 1×, with room to read:** the hero episode, Break down's storyboard, the search results and the loaded view at the end.
- **Secondary steps move fast:**
  - cursor moves and the dwells before secondary clicks run at 2×;
  - from Break down on, the routine clicks (the +, the menus, the query box, the Publish panel, Data, the view) run at 3×;
  - rests run at 10×, and typing at 5× for the query and 15× for the view name.
- **Model waits:** Break down (10 s) and the view read (12 s) become eased time-lapses of 0.6 s and 0.25 s.
- **Silences:** at most 1.3 s between lines, except about 1.5 s either side of "Now search for the one we want". The search setup around it can't shrink further without rushing.

The narration fills the intro: its first five lines run 0:00.3–0:27.1 with gaps of 0.4–1.0 s. The episode therefore plays whole at 1×, and the steps after the Select carry the pace.

The narration is Umbriel with the pro-grounded direction, 137 words. **Edit the narration and beats here.** The take and edit are re-pinned from this section.

**0:00–0:21: The problem, on a recording.** Data → expand `warp_sim_bottles_sample` (0:00.5) → open episode `…daa7587d` (0:02.3, 14.4 s) → Play at 0:04.0. The episode plays whole at 1×:
- the right arm drops the bottle into the red bin at 0:09.4 ("drop" lands on it);
- the left arm drops the can at 0:15.7;
- the arms pull back, and the episode ends at 0:18.8.

The cursor then heads for Curation (0:20.0), which opens already named and over the dataset. "…for you" ends 0.4 s later, and the cut to the graph shot is at 0:21.0.

> Putting a bottle and a can in the bin looks like one task. It's really a few small ones: reach, grab, carry, drop, pull back.

> If your robot keeps missing the bin, you want to train it on more drops. But first you have to find every drop, in every episode.

> Spiral can find them for you.

**0:21–0:27: Basic filtering.** Dataset + (0:21.9) → **Select** (0:22.7) → flip the comparison to *at most* at 0:24.5 (the value jumps to 60.6 s, so all 100 are kept) → drag down to **17 s** (0:25.9–0:27.0), which keeps 11 of 100. The line runs 0:21.3–0:27.1 and ends as the drag lands.

> First, Spiral lets you quickly filter episodes by metadata, like duration or message count.

**0:28–0:35: Break down subtasks.** Select + (0:28.0) → **Break down subtasks** (0:28.6) → the primer plays at 3× → click **Break down 11 episodes** at 0:31.2. The 10 s model wait eases into 0.6 s, and the rows land at 0:33.1 (75 subtasks), as the line ends on "a short description". At 0:34.4 the cursor clicks the hero's placing step, "Lift the bottle and place it into the red bin with the right arm" (0:04.5–0:07.0). The Subtasks lane reads like a storyboard covering the whole episode, and it holds 0.6 s at 1×. Short steps read down their chip like a book spine; the narrowest chip shows no label rather than a column of clipped letters.

> This node splits each episode into smaller tasks and labels each one with a short description.

**0:35–0:47: Find the skill everywhere.** The first half of the line runs 0:35.1–0:36.6, over the storyboard → Break down + (0:36.2) → **Find similar subtasks** (0:37.0) → the query box (0:38.1), where "putting an object into a container" starts → type the query (0:38.4, at 5×) → Search (0:40.5). The results land at 0:41.3: "Found in 11 of 11 episodes", 17 of 75 subtasks. The top match plays at 1×: the hero's own drop, "Lift the bottle and place it into the red bin with the right arm" (91%). The first results screen holds 5.8 s for the next line. Its receptacles read red bin, red bin, black mug, green bin, bin, red bowl, bin, bin, and cup. The cup row is the ninth, partly below the fold, with "cup" visible.

> Now search for the one we want: putting an object into a container.

> It shows up in all eleven episodes, even where one label says bin and another says cup.

**0:47–0:51: Publish.** Publish (0:47.6) → choose Find similar (0:48.7) → name the view `placing_skill` (0:49.4, at 15×) → Publish (0:50.7). The Published flag appears at 0:51.0, and the line ends on "catalog" 0.4 s later. The panel never shows "Name the view." (fixed: it now waits until the name field is left empty or Publish is pressed).

> Now we can publish this curation to the Spiral catalog.

**0:52–0:57: Check it in Data.** Data (0:52.1) opens on the dataset's Overview, with no stale episode. The cut to the data shot is at 0:53.6, on the cursor resting on `placing_skill` → open it (0:53.8). The 12 s read eases into a quarter of a second, and the view loads at 0:55.1: 19 slices in 11 episodes, 37 s kept of 2 min 50 s. The line ends 0.1 s later, and the cut holds on the full app until 0:56.9, without playing a slice.

> Back in Data, the new view is ready for training.

## Verified numbers (September 26–27, current UI)

Break down now covers each episode end to end: one contiguous partition, with no gaps and no overlaps, so approach, idle and retract moments get subtasks too (driver rebuilt 21:58). A walkthrough on that driver found every one of the 11 episodes starting at 0:00.0 and ending at its duration. Figures marked *before full coverage* come from the earlier driver.

- **Dataset:** `dataset_warp_sim_bottles_sample`, 100 simulated, human-teleoperated episodes of 13.6–60.5 s. Keep internal identifiers out of narration, and never call it physical-robot footage.
- **Select:** Duration **at most 17 s** keeps **11 of 100**: the quickest demonstrations, 13.6–16.8 s, each a clean pick-and-drop of two objects into a bin, one by each arm. No round threshold gives exactly 10 (at most 16.7 s keeps 10, at most 16.5 s keeps 9). Kept episodes in recording order: `…daa7587d` (14.4 s), `…921e085e` (15.7), `…ed749578` (16.0), `…05643eb4` (13.6), `…b5345bf9` (16.8), `…292c5cb6` (16.7), `…9625f631` (16.5), `…caf9346f` (16.4), `…ce815d7d` (14.3), `…57eed17c` (14.8), `…e0e8a790` (15.2).
  - The Select starts at "Duration at least 0 s". Switching it to "at most" now moves the value to the other end of the strip (60.6 s), so the Select still keeps all 100 episodes. Dragging down to 17.0 s then leaves out 89.
  - Every episode records exactly 120 messages per second and 4 streams, so message count, channel count and file size only restate the duration.
- **Hero:** episode `…daa7587d` (catalog id `01a0b63c7b8076df9a857868daa7587d`, rank 2), 14.4 s. The right arm drops a brown bottle into the red bin at about 4.5–7.5 s; the left arm then drops a blue can in at about 9.5–12 s. It is the first kept episode, so Break down opens it first. Before full coverage, its right-arm placing step was the top or near-top Find similar match in every run (88–92%); with full coverage, other episodes' drops rank first. In Data, the right wrist camera shows the bottle dropping into the red bin.
- **Break down subtasks** (11 episodes, top camera, 0.5 s samples), with full coverage: 76, 77, 84, 84, 76, 77, 80 and 79 subtasks in eight runs, 5–8 per episode. Since the 23:07 restart the descriptions are one style: short lowercase phrases with no trailing period (Studio capitalizes the first letter for display). *Before full coverage:* 55, 47 and 50, 3–10 per episode. Descriptions and boundaries differ between runs, and the model sometimes misreads an action; in the first take it said the hero's right arm "repositions the red cup" when it drops the bottle. It names the receptacle a bin, container, cup, bucket, basket, bowl, mug, pot or dustpan, depending on the run and the episode. In takes 5–7 it described the hero as handing the bottle from the right arm to the left; the narration makes no claim about the hero's steps.
  - The hero in the full-coverage walkthrough: 0–3.0 "move the right arm toward the bottle while the left arm moves toward the can"; 3.0–4.5 "grasp the bottle with the right arm and the can with the left arm"; 4.5–7.5 "place the bottle into the red cup using the right arm"; 7.5–9.0 "retract the right arm to rest position"; 9.0–11.5 "place the can into the red cup using the left arm"; 11.5–14.4 "retract both arms to rest position".
- **Find similar** "Put an object into a container", cutoff 50%, with full coverage: 20 of 76 subtasks in 11 of 11 episodes (walkthrough), 16 of 77 (take 1), 18 of 84 (take 2), 20 of 84 (take 3), 19 of 76 (take 4), 15 of 77 (take 5), 18 of 80 (take 6) and **17 of 79 in 11 of 11 (take 7, the one in the cut)**.
  - Take 7 ranks …caf9346f "Place the blue cylinder into the pot with the right arm" (92%) first. Its first results screen names the receptacle pot, purple bin, green bucket, purple bin, black container, bin, bucket, red cup and bin, so "bin" and "cup" are both on screen.
  - Take 5 could not be used: its first results screen said purple bucket, pot, container, red cup, black container, red cup, cup and pot, with no "bin", so the results line would have been false. In take 6, Break down called the hero's drop "Insert the bottle into the cup", which the driver's step match did not cover, so the take had no storyboard beat. The match now also accepts insert, pot, basket and mug.
  - Take 4 ranked …b5345bf9 "place the marker into the bin with the left arm" (91%) first. Its first results screen named the receptacle bin, bin, bin, cup, green cup, green cup, bin, purple bin and bin.
  - Take 3 ranked …ce815d7d "drop the light block into the container" (93%) first, …ed749578 "place the fish toy into the red cup" (92%) second, and the hero's "place the bottle into the red cup with the right arm" (91%) fifth.
  - Take 3's first results screen named container, red cup, brown cup, bin, red cup, bin, cup, brown cup and bin. No label said pot or bowl, which is why the results line says "bin" and "cup". Take 2 did say "pot", but Break down called the same flowerpot a "cup" in take 3. *Before full coverage:* 19 of 55 in 11 of 11, 15 of 47 in 9 of 11, and 17 of 50 in 11 of 11. Matches score 78–93%; nearly everything else scores 25% or lower, so the cutoff sits in a gap.
  - Queries that name the bin do worse, because the descriptions rarely say "bin": "Place the object into the bin" kept 8 subtasks in 5 episodes, and "Drop the object into the container" kept 16 in 10 episodes.
  - Starting from a row's own Find similar makes the row's words the query ("Right arm grasps the bottle and places it into the red container"), which matched only 1 other subtask.
- **Group similar** (for comparison, not in the video): 7 proposed categories (Grasping 18, Placing 12, Reaching 8, Relocating 8, Transporting 5, Retracting 3, Bimanual Transfer 1). Placing is split between Placing and Relocating, and keeping just placing means unticking six categories. Find similar tells the story in one query.
- **Published view:** re-runs Break down (a model call per episode) and the similarity judgments on every read. Spiral caches no model answers, so **every read shows different words and boundaries than Curation did, and than the previous read**. Before full coverage, three reads of one Find similar view returned 16, 16 and 18 intervals, always across all 11 episodes. With full coverage, the reads returned 18 intervals in 10.8 s (take 1), 17 in 18.6 s (take 2) 16 in 14.3 s (take 3), 16 in 21.2 s (take 4), 17 in 17.9 s (take 5), 18 in 9.3 s (take 6) and 18 in 11.1 s (take 7), always across 11 episodes. Take 7's read kept 27 s of 2 min 50 s. It listed the hero's slices as "Place the bottle upright into the bin with the right arm" (0:04.5–0:07.0, 88%) and "Place the can into the bin with the left arm" (0:10.0–0:11.5, 89%), where Curation had shown "Place the bottle into the bucket with the left arm" (0:10.5–0:11.5). The cut no longer plays a slice. In earlier takes, for the hero, Curation showed "Right arm grasps the bottle and places it into the red container" (0:04.5–0:08.0). The reads showed "Left arm places the bottle inside the cup" (0:10.5–0:12.0); "Place the bottle into the trash bin with the left arm" (0:08.0–0:12.0); and "Right arm places the bottle into the red bucket" (0:04.5–0:07.5) with "Left arm places the blue and brown can into the red bucket" (0:10.0–0:12.0). The user accepted this for now. The narration never claims Data shows the same slices or words as Curation.

## Measured timings (headless Brave, 1440 × 900)

| Step | Time |
|---|---|
| Choose dataset, add Select | instant |
| Break down primer animation | about 2.5 s |
| Break down 11 episodes | 9.7 s, 20 s, 9.0 s, 20 s, 14.7 s, 9.5 s, 14.1 s and 19.1 s with full coverage (16 s, 21 s and 9 s before it); rows stream in as episodes answer |
| Find similar search (typed query) | 1–3 s |
| Group similar (propose categories, classify) | 5.6 s |
| Publish (saves, creates the view, records it) | 0.5–1 s |
| Data read of the 11-episode view | 10.8 s, 18.6 s, 14.3 s, 21.2 s, 17.9 s, 9.3 s and 11.1 s with full coverage (13 s, 17 s and 22 s before it) |
| Open a slice → episode tab playing | about 1.3 s |

Compared with video 1 (115 s of scoring and a 119 s view read), almost nothing needs compressing.

## Fixes made for this video

- **Index rows** (Break down, Find similar, Group similar): a row's facts line no longer cuts off its time ("0:04.5 – 0:08.(") at 1440 px. Facts that don't fit beside the score take a second line. `crates/frontend/styles/curation-review.css`.
- **Data shows similarity the way Curation does:** "90%" rather than "0.90". `activities/curation/curated_view.rs` (`CuratedView::reading`), `activities/curation/annotate/subtask.rs` (`SubtaskRow::is_similarity`, `similarity_label`).
- **The empty band under a curation's timeline is gone.** A node's timeline tile is sized so its rows lead, and a row of subtasks used to leave most of it empty. The Subtasks row now fills the tile, and its spans read top-down like a schedule. Changing the arrangement would not have worked: in these rooms any taller recording band turns the one-over-two cameras into a column of three, or letterboxes them. `crates/frontend/styles/curation-review.css`.
- **Narrow subtask chips say their words, or nothing.** In a curation, a chip too narrow for its label runs it down the chip like a book spine. A chip under 21 px would clip even that one line of letters into a column of dots ("riaht"), so it shows no label, and the readout beside the lane names the step. Elsewhere, including Data's episode tab, a word breaks at a hyphen; only a chip under 18 px shows no label. `crates/frontend/tailwind.css`, `crates/frontend/styles/curation-review.css`. Stills: `fixes/narrow-chips-before.png`, `fixes/narrow-chips-after.png`.
- **The Publish panel no longer asks for a name before you've had a chance to type one.** Ticking a view used to show "Name the view." at once. Now a ticked view with an empty name says so only after its field is left empty or Publish is pressed; a typed name that can't be used is flagged as it is typed. `activities/curation/graph/publish.rs` (`PublishChoice::shown_problem`), `crates/ui/src/controls/field.rs` (`TextInput` gained `onblur`). Stills: `fixes/publish-name-before.png`, `fixes/publish-name-after-ticked.png`, `fixes/publish-name-after-attempt.png`.
- **Select comparison flip:** switching "at least" to "at most" on a condition that leaves nothing out used to flash "Kept 0 · Nothing passes this rule". It now keeps every episode, from the other end of the strip. A condition that already leaves episodes out keeps its value. `activities/curation/select/rule.rs`, `activities/curation/threshold.rs` (`Threshold::ends`).
- Screenshots before and after: `exports/demo-series-2026-09-26/02-break-down-subtasks/fixes/`.

## Recording evidence

- **Shots:** `exports/demo-series-2026-09-26/02-break-down-subtasks/shots.json` names the take chosen for each shot. The recordings and their logs are in `shots/`.

  | Shot | Take | Recorded |
  |---|---|---|
  | intro | `shots/0927-193400-intro.mp4` | September 27, 19:34 |
  | graph, breakdown, search, publish | `shots/0927-190124-graph+breakdown+search+publish-v1.mp4`, one recording | September 27, 19:01 |
  | data | `shots/0927-193455-data.mp4` | September 27, 19:34 |

  - **The group's variants:** two were recorded (`--variants=2`), and `shots.json` keeps the second under `others`. Variant 2 failed the search shot's check: its first results screen never said "bin".
  - **Joins:** `uv run ../kit/shots.py joins shots.json` diffs the last frame before each cut against the first frame after it.
    - intro → graph: 300 px differ (0.006 % of the frame, peak 18 of 255).
    - publish → data: 275 px differ (peak 18).
    - Both are encoder noise. Take 7's data shot couldn't be kept: its first frame still showed the Published toast, which the new publish shot waits out.
  - **Capture:** 2880 × 1800 lossless PNG frames from the 1440 × 900 page, tagged sRGB → BT.709 (kit/README.md, "Recording and colour"). Brave ran with `--window-size=1440,1037 --force-device-scale-factor=2`, and the driver defaults to `--dpr=2`. The screencast delivered 33–46 distinct frames per second, into 30 fps files.
  - **Driver:** `take-02.mjs` lists the six shots for `../kit/shots.mjs`, with the kit's `HumanInput` and a seed per shot. `node take-02.mjs --shot=graph` records one shot, and the shots before it replay off camera first.
    - Break down's subtasks differ on every run, and search and publish show them, so those three are one group, always recorded together.
    - The search shot's check requires "bin" and "cup" on the first results screen, because the results line quotes both.
    - The breakdown shot's check requires a placing step for the hero, so the storyboard beat exists.
  - **Errands, off camera:**
    - Starting the curation over the dataset and naming it: the intro's setup.
    - Closing the hero's episode tab: the graph's setup.
    - The Published toast clears before the publish shot's last frame.
  - **Logs:** `shots-input.log` is every chosen shot's log on the edit's clock. It has every move, click, drag, key, wait and mark, plus what the app answered: counts, the top match, the hero step, the first results screen's words, and the view's facts and first rows.
  - **Earlier takes:**
    - Take 7 (`raw-take.mp4`, `take-input.log`) was sliced into the first take of every shot; its driver is kept as `take-02-whole.mjs`.
    - Takes 1–6 are in `take-1/` to `take-6/`.
    - Every session that publishes first deletes the previous "Placing skill": publishing a view name that already exists replaces the view, and deleting a curation drops its views by name.
  - Headless Brave draws no native menu, so the driver flips the comparison select programmatically while the drawn cursor presses it.
  - **Dry run:** `node take-02.mjs --dry` drives the shots without recording, under its own curation name ("Placing skill dry run") and view name. Delete that curation afterwards, which drops its view.
- **Rough cut:** `rough-cut-voiced.mp4`, **56.9 s** at 2880 × 1800 and 30 fps. `edit_from_log.py` writes `edit.json` (and `shots-input.log`) from `shots.json`, and `../kit/edit.py render edit.json` renders it. The beat times are in [Narration and screen action](#narration-and-screen-action).
  - **Joins:** plain cuts at 0:21.0 and 0:53.6, each on a still frame that matches. There are no dissolves.
  - **Zones:**
    - The hero episode plays whole at 1×.
    - Secondary clicks run their move and dwell at 2×. From Break down on, the routine clicks run at 3×. A long wait between arriving and pressing runs as a rest.
    - Other cursor moves run at 2×, and rests of 0.4 s or more after the intro at 10×.
    - Typing runs at 5× for the query and 15× for the view name. The slider drag runs at about 1.3×, the primer at 3× and the search at 5×. Data's page switch runs at 2×.
    - The Break down wait (10 s) eases into 0.6 s, and the view read (12 s) into 0.25 s.
    - At 1×: the hero episode, the hero's step (0.6 s after the click), the first results screen (5.8 s), and the loaded view (1.8 s).
  - **Voice:** Umbriel pro-grounded. Every line is cut 0.08 s either side of its speech and pinned to the action it describes.
    - Five lines come from the full take `voice-tests/norm/series02-v3b.wav`.
    - The results line is `series02-v3-s07-a.wav`.
    - The filter line is `series-filter-a.wav`.
    - The bridge is `series02-v5-bridge-c.wav`, 125 Hz, between line 2 (136 Hz) and the filter line (113 Hz). Take a read at 197 Hz.
    - The publish and Data lines are `series02-v4-publish-b.wav` and `series02-v4-data-b.wav`.
- **Remotion render:** `remotion/out/video-2.mp4`, 59.05 s with the title and end cards.
  - **Camera:** one zoom, on the hero's cameras while the episode plays, and out before the move to Curation. Everything else is a spotlight at the full frame, including the Find similar node and the Published flag. After placing_skill is clicked, the camera stays on the full app to the end.
  - **Checks:** colour tags sRGB transfer, BT.709, limited range; loudness −15.95 LUFS integrated, true peak −4.5 dBTP; on-device speech recognition heard 135 of 137 words in order ("first" written "1st", "short" heard as "shore"); captions a median 45 ms from the heard word.

## Before recording the native take

- Start Curation from a fresh page so it opens on a blank graph, and start Data with nothing open.
- Episodes remember their layout per dataset in `localStorage`. If a layout was dragged by hand, reset it so the designed one shows.
- Safari draws its own menu for the comparison select: choose "at most" there. The headless take sets it programmatically, because headless Brave draws no native menu.
- The Curated rail also lists `long_demos_steady` (video 1) and the older `similar_task_view`, marked Changed. The user decides whether they stay for the recording.
