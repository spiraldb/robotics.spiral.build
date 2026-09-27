# Series video 2: Break down subtasks

Drafted September 26, 2026, against the current Studio. **Target: about 60 seconds.** It follows the series pattern: start from a problem, curate the dataset in Spiral, publish the curation, and open the published curation in Data to check it. The graph is Dataset → Select → Break down subtasks → Find similar subtasks → Publish → Data.

**This round is for story and pacing.** The proof-of-concept take was recorded with the headless Brave driver, so its cursor is a drawn overlay and its typing is synthetic. The native Safari take comes once the story is approved (see [video 1](01-score-progress.md#recording-evidence)).

Voice: Umbriel with the professional direction ([voice tests](../../../exports/demo-series-2026-09-26/voice-tests/README.md)).

## The story

Episodes are whole-task demonstrations: a person teleoperating the simulated robot drops every object into the bin. To train or evaluate one skill, such as placing an object in the bin, you need that skill's boundaries in every episode, and labeling them by hand is slow and inconsistent. Spiral breaks every episode into timestamped subtasks, finds the same skill across the episodes, and publishes just those slices as a view that Data can open and play.

## Narration and screen action

TAKE-PENDING

## Verified numbers (September 26, current UI)

**Measured before `spql.ai.breakdown_task` changes to cover each episode end to end** (one contiguous partition, with no gaps and no overlaps, so approach, idle and retract moments get subtasks too). Everything below about Break down, Find similar and the published view must be measured again once the rebuilt driver is loaded. The Select figures don't depend on it.

- **Dataset:** `dataset_warp_sim_bottles_sample`, 100 simulated, human-teleoperated episodes of 13.6–60.5 s. Keep internal identifiers out of narration, and never call it physical-robot footage.
- **Select:** Duration **at most 17 s** keeps **11 of 100**: the quickest demonstrations, 13.6–16.8 s, each a clean pick-and-drop of two objects into a bin, one by each arm. No round threshold gives exactly 10 (at most 16.7 s keeps 10, at most 16.5 s keeps 9). Kept episodes in recording order: `…daa7587d` (14.4 s), `…921e085e` (15.7), `…ed749578` (16.0), `…05643eb4` (13.6), `…b5345bf9` (16.8), `…292c5cb6` (16.7), `…9625f631` (16.5), `…caf9346f` (16.4), `…ce815d7d` (14.3), `…57eed17c` (14.8), `…e0e8a790` (15.2).
  - The Select starts at "Duration at least 0 s". Switching it to "at most" now moves the value to the other end of the strip (60.6 s), so the Select still keeps all 100 episodes. Dragging down to 17.0 s then leaves out 89.
  - Every episode records exactly 120 messages per second and 4 streams, so message count, channel count and file size only restate the duration.
- **Hero:** episode `…daa7587d` (catalog id `01a0b63c7b8076df9a857868daa7587d`, rank 2), 14.4 s. The right arm drops a brown bottle into the red bin at about 4.5–7.5 s; the left arm then drops a blue can in at about 9.5–12 s. It is the first kept episode, so Break down opens it first. Its right-arm placing step was the top or near-top Find similar match in every run (88–92%). In Data, the right wrist camera shows the bottle dropping into the red bin.
- **Break down subtasks** (11 episodes, top camera, 0.5 s samples): 55, 47 and 50 subtasks in three runs, 3–10 per episode. Descriptions and boundaries differ between runs. The model names the receptacle a bin, container, cup, bucket, bowl, pot or dustpan, depending on the run and the episode.
- **Find similar** "Put an object into a container", cutoff 50%: 19 of 55 subtasks in 11 of 11 episodes (walkthrough); 15 of 47 in 9 of 11 (second run); 17 of 50 in 11 of 11 (dry run). Matches score 78–92%; nearly everything else scores 25% or lower, so the cutoff sits in a gap.
  - Queries that name the bin do worse, because the descriptions rarely say "bin": "Place the object into the bin" kept 8 subtasks in 5 episodes, and "Drop the object into the container" kept 16 in 10 episodes.
  - Starting from a row's own Find similar makes the row's words the query ("Right arm grasps the bottle and places it into the red container"), which matched only 1 other subtask.
- **Group similar** (for comparison, not in the video): 7 proposed categories (Grasping 18, Placing 12, Reaching 8, Relocating 8, Transporting 5, Retracting 3, Bimanual Transfer 1). Placing is split between Placing and Relocating, and keeping just placing means unticking six categories. Find similar tells the story in one query.
- **Published view:** re-runs Break down (a model call per episode) and the similarity judgments on every read. Spiral caches no model answers, so **every read shows different words and boundaries than Curation did, and than the previous read**. Three reads of one Find similar view returned 16, 16 and 18 intervals, always across all 11 episodes. For the hero, Curation showed "Right arm grasps the bottle and places it into the red container" (0:04.5–0:08.0). The reads showed "Left arm places the bottle inside the cup" (0:10.5–0:12.0); "Place the bottle into the trash bin with the left arm" (0:08.0–0:12.0); and "Right arm places the bottle into the red bucket" (0:04.5–0:07.5) with "Left arm places the blue and brown can into the red bucket" (0:10.0–0:12.0). The user accepted this for now. The narration never claims Data shows the same slices or words as Curation.

## Measured timings (headless Brave, 1440 × 900)

| Step | Time |
|---|---|
| Choose dataset, add Select | instant |
| Break down primer animation | about 2.5 s |
| Break down 11 episodes | 16 s, 21 s and 9 s in three runs; rows stream in as episodes answer |
| Find similar search (typed query) | 1–3 s |
| Group similar (propose categories, classify) | 5.6 s |
| Publish (saves, creates the view, records it) | about 1 s |
| Data read of the 11-episode view | 13 s, 17 s and 22 s |
| Open a slice → episode tab playing | about 1.3 s |

Compared with video 1 (126 s of scoring and a 119 s view read), almost nothing needs compressing.

## Fixes made for this video

- **Index rows** (Break down, Find similar, Group similar): a row's facts line no longer cuts off its time ("0:04.5 – 0:08.(") at 1440 px. Facts that don't fit beside the score take a second line. `crates/frontend/styles/curation-review.css`.
- **Data shows similarity the way Curation does:** "90%" rather than "0.90". `activities/curation/curated_view.rs` (`CuratedView::reading`), `activities/curation/annotate/subtask.rs` (`SubtaskRow::is_similarity`, `similarity_label`).
- **The empty band under a curation's timeline is gone.** A node's timeline tile is sized so its rows lead, and a row of subtasks used to leave most of it empty. The Subtasks row now fills the tile, and its spans read top-down like a schedule. Changing the arrangement would not have worked: in these rooms any taller recording band turns the one-over-two cameras into a column of three, or letterboxes them. `crates/frontend/styles/curation-review.css`.
- **Narrow subtask chips say their words.** In a curation, a chip too narrow for its label runs it down the chip like a book spine. Elsewhere, including Data's episode tab, a word breaks at a hyphen; only a chip under 18 px shows no label. `crates/frontend/tailwind.css`, `crates/frontend/styles/curation-review.css`.
- **Select comparison flip:** switching "at least" to "at most" on a condition that leaves nothing out used to flash "Kept 0 · Nothing passes this rule". It now keeps every episode, from the other end of the strip. A condition that already leaves episodes out keeps its value. `activities/curation/select/rule.rs`, `activities/curation/threshold.rs` (`Threshold::ends`).
- Screenshots before and after: `exports/demo-series-2026-09-26/02-break-down-subtasks/fixes/`.

## Recording evidence

TAKE-PENDING

## Before recording the native take

- Start Curation from a fresh page so it opens on a blank graph, and start Data with nothing open.
- Episodes remember their layout per dataset in `localStorage`. If a layout was dragged by hand, reset it so the designed one shows.
- Safari draws its own menu for the comparison select: choose "at most" there. The headless take sets it programmatically, because headless Brave draws no native menu.
- The Curated rail also lists `long_demos_steady` (video 1) and the older `similar_task_view`, marked Changed. The user decides whether they stay for the recording.
