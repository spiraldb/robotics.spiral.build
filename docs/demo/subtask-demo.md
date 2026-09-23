# Demo: Find the same subtask across episodes

### Overall flow
Turn a robot dataset into searchable moments: inspect one demonstration, discover its subtasks, and find the same operation across other episodes.

1. **Inspect an episode.** Open a cloth-folding demonstration and play its camera and robot-state views.
2. **Break it into subtasks.** A vision-language model reads sampled frames and produces timestamped descriptions: grasp the corners, make the first fold, smooth the cloth. Show these as clickable timeline segments.
3. **Annotate every episode.** Run the same process across the dataset and save each episode's subtask intervals.
4. **Find similar subtasks.** Click **Find similar** on “make the first fold” and rank every other subtask in the dataset by how likely it describes the same operation. Ranking runs on the fly with jev, the decision model Curation already uses; nothing is precomputed. The matches light up across an episode map, showing where in each episode the operation happens.
5. **Compare the results.** Show the ranked grid of video slices, best match first. Each plays only its subtask interval and identifies the source episode and time range. Clicking a result returns to its original context.
6. **Group and profile.** Watch every episode split into its subtasks and pour into a handful of named categories. Open one to see how it differs from the rest of the dataset: how long it takes, when it happens, what comes before and after it, and how the arms move.

### Proposed UI

**Explore** adds a dataset → measurement → exploration graph to Workflow, alongside the existing **Agent** tab.

Start with one dataset node. A small **+** centered on its bottom edge opens a measurement menu with **Break down subtasks**. Choosing one creates a connected node. Use the same quiet bottom-center **+** on measurement nodes for their supported child actions; no labeled button row.

**Chosen layout: [B — graph beside results](../../.impeccable/mocks/workflow-build-b.png).** Keep the graph in a resizable left pane and a larger workspace for the selected node on the right. Match the current Studio design: compact tabs and controls, white panels, pale dotted canvas, thin borders, and blue selection. The mockup's imagery and counts are illustrative.

Latest interaction mockups: [search results](../../.impeccable/mocks/workflow-build-b-v2.png) and [subtask output with the + menu open](../../.impeccable/mocks/workflow-build-subtasks-v2.png).

Selecting a node changes the right workspace:

- **Dataset:** browse and inspect episodes.
- **Break down subtasks:** configure and run annotation, then inspect the output table and annotated timeline. The node summarizes its run state and output; the table uses the schema below.
- **Find similar subtasks:** a child node with a search box and a ranked grid of camera slices, best match first. The query is the selected subtask's description or typed text. Every subtask row appears in the ranking with its jev probability; there is no cutoff. Each result shows its description, source episode, interval, and probability. Every subtask row in every Explore output also carries a **Find similar** action that runs this search with that row as the query ([below](#find-similar-from-any-subtask)).
- **Group similar subtasks:** a child node showing 4–8 named categories with representative slices and member counts over the [episode map](#episode-map). Opening a category shows its [profile](#category-profile), meaning how it differs from every other subtask, above its members, most typical first. The categories come from one LLM pass over the concatenated descriptions; membership comes from one jev Choice per subtask row. Rows are grouped, not episodes, so an episode may appear in several categories.

Each measurement's **+** exposes only the child actions its output supports. Search and grouping read the saved subtask table and never rerun annotation. Search persists only its query, never its ranking; grouping persists its categories and assignments. Keep video results in the workspace so graph nodes remain small and readable.

**Implementation order:** implement and persist the exploration graph first, clearly marking measurements and explorations that cannot execute yet. Add annotation, then jev search and grouping.

### Annotation

**Orchestration lives on the frontend, following the Workflow agent.** An Explore measurement controller owns episode iteration, contact-sheet preparation, prompts, model and jev requests, result validation, and progress. Frames are selected and decoded natively by Spiral SQL through the episode query endpoint, the way Curation samples a camera; the browser only composes them into contact sheets. The backend stores one versioned exploration per graph and never interprets it. The frontend reads Studio-wide credentials, constructs authenticated provider requests, and handles provider responses and retries. The backend relays HTTP through `/api/proxy`, preserving streaming and cancellation without interpreting model requests or selecting credentials. This measurement is a fixed pipeline with one annotation call per episode. Closing or reloading the browser stops its orchestration; already-persisted rows remain available.

**One annotation call per episode, up to eight episodes at once.** Start with one selected camera and the **latest Gemini Flash** (`google/gemini-3.8-flash` on OpenRouter as of 2026-09-21; Macrodata used 3.5 Flash) as the model. Our pipeline adapts [Macrodata's frame-packing and single-pass approach](https://macrodata.co/blog/annotating-robot-video-subtasks):

1. Select frames every 0.5 seconds. One cheap query gives the camera's packet count and recorded span; each sample time becomes a frame index, and one query per 60 frames assembles the packet stream, selects those frames, and decodes only them. Batches stay small because a query response is capped at 4 MiB and a decoded frame returns as a 640px JPEG. The browser resizes the returned frames to 224px-wide tiles, draws episode-relative timestamps, and arranges 20 frames into each 4 × 5 image grid.
2. Send **all grids for that episode in chronological order in one request**, with the task instruction and a structured output schema, at low reasoning effort: on one ABC-130k episode, low cost $0.0066 and 2.5 seconds against $0.059 and 33 seconds at medium, with comparable subtasks, and this model refuses to disable reasoning entirely. Ask for meaningful manipulation events, returning `{start_time, end_time, description}` for each. The model infers durations from changes in the action; there is no fixed subtask length.
3. Validate the returned intervals in code: nonempty descriptions, finite times, `0 ≤ start_time < end_time ≤ episode_duration`, chronological order, and no overlaps for this initial single-sequence annotation. Report invalid output before saving.
4. Persist the episode's subtask rows. Search and grouping read those rows later; neither reruns video annotation.

**Grid boundaries do not split tasks.** Each grid packages roughly 10 seconds of samples; the model's context is the whole episode. For example, a fold starting at 8 seconds and finishing at 13 seconds spans two grids but produces one `[8, 13)` row. Keep timestamps continuous across grids and explicitly tell the model that sheet edges are not action boundaries. Sampling at 0.5 seconds gives approximate temporal evidence, not frame-accurate boundaries.

Macrodata found that separate calls per grid introduced artificial cuts, even with overlap. Its optional second pass relabels predicted segments with neighboring context; we defer that until the single-pass labels prove insufficient. Very long episodes that exceed request limits need a separate chunking/reconciliation design, outside the initial demo.

**Reuse existing components; add the annotation-specific pieces:**

| Existing component | Planned reuse |
| --- | --- |
| Dataset/episode selection and Curation's native frame sampling | Select and decode frames at 0.5-second timestamps with Spiral's video functions over `/api/queries/episode`, then draw contact sheets on a canvas from the returned JPEGs. |
| Browser-owned Workflow controller and shared `ModelClient` | Reuse the task lifecycle and frontend provider handling; the measurement owns its own progress and requests, independently of the chat conversation. `ModelClient::decide` already sends jev Decisions requests for Curation's rule interpreter; search and grouping reuse it. |
| Curation's jev rule interpreter (`RuleInterpreter`) | Copy its state-plus-questions construction and code-side binding. Search and grouping build their own questions. The shared `DecisionResponse` needs Noul and probability readers; its existing `choice` reader enforces a 0.5 confidence gate that search and grouping do not use. |
| Studio-wide `/api/credentials` and `/api/proxy` | Obtain configured credentials and relay external HTTP requests. Annotation prompts, authorization headers, and response parsing stay on the frontend. Add structured output and Gemini configuration; Gemini support in the provider client must be verified, not assumed. |
| `EpisodeCamera`, `EpisodeTransport`, shared `Timeline` | Reuse playback, seeking, and interval playback; add a labeled subtask track. Extract Curation's `EpisodeReview` composition for sharing instead of building another player. |

New work remains: the frontend measurement controller, contact-sheet assembly, the annotation prompt/schema and validator, the jev search and grouping question builders with their result readers, the category-proposal prompt, and subtask-table persistence. These are proposed additions, not capabilities already implemented in the repo.

### Search and grouping with jev

**No embeddings.** Both explorations use [jev](https://docs.typesafe.ai/introduction), TypeSafe's decision model, through the OpenRouter Decisions transport that Curation's rule interpreter already uses: model `typesafe/jev-1.13`, `POST https://openrouter.ai/api/alpha/decisions`, relayed through `/api/proxy`. jev generates no text. It evaluates typed questions against one `state` and returns calibrated probabilities. A **Noul** returns the probability that a yes/no question is true. A **Choice** picks one of up to 255 options and returns a probability per option plus a confidence. Every question in a request is evaluated in parallel against the same state, so batching many questions adds little latency. A request holds up to 64k tokens, with 32k for the state plus the longest question. Input costs $0.042 per million tokens; output is free. Instructions reference parts of a structured state with backticked paths such as `` `candidates.r17` ``. Both explorations need the OpenRouter credential, as rule interpretation does; with one OpenRouter key, the annotation model and jev share it.

**Find similar subtasks ranks on the fly.** This follows TypeSafe's [re-ranking cookbook](https://docs.typesafe.ai/cookbooks/rerank_typesafe.md): one Noul per query–candidate pair, sorted by probability.

1. The query is the selected subtask's description, or text typed in the search box.
2. Put the query and a batch of candidate rows in the state, keyed by row: `{"query": "make the first fold", "candidates": {"r17": "fold the cloth in half", ...}}`. Include the dataset's task instruction when one exists.
3. Ask one Noul per candidate. Instructions: “Does `candidates.r17` describe the same manipulation operation as `query`?” Criteria `true`: “the same operation on the same kind of object, even if worded or executed differently.” Criteria `false`: “a different operation, a neighboring step, or only the same object or scene.”
4. Read each answer's `noul` as that row's relevance. Batch rows to stay within the request limit and run batches concurrently. Noul probabilities are comparable across requests, so one sorted list covers the whole table.
5. Show every row sorted by probability, best first, with its probability. There is no cutoff or threshold; the user scrolls. Load thumbnails lazily and play a slice on selection, because each playing slice decodes its own episode.

No ranking is persisted; a new query is one more round of requests. Sizing: a 2,000-row table at about 400 candidates per request is 5 concurrent requests of roughly 30k tokens each, well under a cent, with sub-second latency per request. The alternative in TypeSafe's [line-by-line search cookbook](https://docs.typesafe.ai/cookbooks/semantic_find.md), one Choice over all candidates, caps at 255 options and returns probabilities that sum to 1 per request, so batches would not be comparable. Prefer the Noul per pair. The cookbook sends one request per pair; if batched Nouls rank measurably worse, shrink the batches.

**Group similar subtasks: the LLM proposes categories, jev assigns rows.**

1. Concatenate the distinct subtask descriptions (dedupe exact text, keep counts) into one request to the annotation LLM. Ask for 4–8 categories with structured output: a short name and a one-sentence definition that distinguishes it from the other categories. Validate in code: 4–8 entries, unique names, nonempty definitions.
2. Assign each subtask row with one jev Choice. State: `{"categories": {...}, "subtasks": {"r0": "...", ...}}`. Instructions per row: “Which category does `subtasks.r0` belong to?” Criteria: the category names mapped to their definitions. Batch rows per request as in search.
3. Every row goes to its highest-probability category. There is no “other” option and no confidence gate: the categories were derived from these same descriptions, so the list is complete by construction. Keep each answer's probability, confidence, and runner-up category to order members, most typical first, and to let the user spot ambiguous rows.
4. Persist the category set and the per-row assignment. Renaming, merging, or editing a category definition reruns only the jev assignment, which is cheap. Regenerating categories reruns the LLM pass.

### Episode map

**Search and grouping share one picture of the whole annotation.** Each measured episode is one thin row, and each of its subtasks is a segment placed by its interval. Every row spans the full width, so a segment's position shows when it happens in its episode, and episodes of different lengths line up by phase; the profile reports absolute durations. Segments stay grey until grouping colors them by category, and search lights them by probability. Clicking a segment selects that row; in a grouping, a segment of another category opens that category first. The map replaces the per-episode coverage strip by turning each strip cell into a lane that shows its subtasks. Lanes are 6 px tall and flow into as many columns as the workspace holds, so 40 episodes take about 100 px at 1440 px wide. Lanes wait for one episode-listing query that supplies the recorded lengths; an episode without a length has no lane. Thousands of episodes would need a summarized map, which is outside this demo.

The map sits between the question and the rows. In search it follows the query and the query card and precedes the ranking. In grouping the order is the status line, the map, the category band, then the open category, so the band reads as the map sorted.

### Find similar from any subtask

**Any row can be the query.** Every `SubtaskEntry` in Break down subtasks, Find similar, and Group similar gets a trailing **Find similar** icon button. It appears on hover and focus and always shows on the chosen row. The evidence header gets the same action beside **Open in context**. Clicking it selects the annotation's Find similar node, or adds one if there is none, sets the row's description as the query, and runs the search immediately. The source row is pinned above the ranking as the query card and left out of the ranking, where it would only rank itself first. `SubtaskSearch` records the source row (episode and `start_time`) beside the query text, so reopening the node shows the same query card. Typing different words clears it. The search workspace asks whatever question the document holds whenever it differs from the one on screen, so opening the node, asking from a row elsewhere, and discarding an edit all rerun it the same way. A row's button is a tab stop only beside the row that holds the index's tab stop, so it adds no presses of Tab.

### Category profile

**Opening a category shows how it differs from everything else.** The member list shows what a category contains; the profile shows what makes it distinctive. The profile leads the open category's index, above the members, and stays inside the index-beside-recording frame. Every dot in the profile is a member, and clicking a dot plays that member in the evidence column. The baseline is every other subtask row in the same grouping.

1. **Headlines.** Up to three sentences: the metrics that separate the category most, ranked by Cliff's delta (the share of member–nonmember pairs where the member is larger, minus the share where it is smaller), then the neighbour it keeps most reliably. Each metric sentence states both medians. Code writes them, not an LLM.
2. **Metric rows.** One row per metric, most distinctive first. Members appear as dots in the category's hue, with every other row as grey dots beneath, and a tick at each median. Every dot is drawn, so an 11-row category shows 11. Duration's axis stops at the table's 95th percentile so a few long rows do not squeeze the rest. Joint ranges would collapse into one strip with a cell per joint, colored by how the category's median range compares with the rest's; a cell would expand into its own dot row.
3. **Neighbors.** Two stacked bars in category hues show what comes before and after members within their episodes, read from the order of the subtask table.
4. **Map focus.** The episode map fades every other category, so it shows at a glance where this one happens.

**Built so far: duration, phase, repeats, and neighbours** (`explore/similar/profile.rs`). The motion metrics below are not built; they wait on the `LAG` decision under **Spiral gaps**.

| Metric | Source | Meaning |
| --- | --- | --- |
| Duration | subtask table | `end_time − start_time` |
| Phase | subtask table, episode duration | start time as a fraction of the recorded episode |
| Repeats | subtask table | members per episode that has any |
| Hand travel, per arm | arm state `pose` | end-effector path length over the interval |
| Hand speed | arm state `pose` | both hands' travel ÷ duration |
| Lowest hand | arm state `pose` | minimum end-effector height, in the arm's base frame |
| Right-hand share | arm state `pose` | right-hand travel ÷ both hands' travel |
| Joint range, per joint | arm state `position` | max − min of each joint over the interval |

**Measured on the abc130k sample.** On 2026-09-23 these metrics were computed over the saved grouping's 263 rows, with one motion statement per episode run by hand. The built profile says the table-derived half for Folding in these words: "Runs longer: 13.8 s median, against 5.5 s for other subtasks. Comes early: members start 33% of the way into their episode, others 44%. Most often followed by Pick and Place (48%)." With motion, the leading headlines would be:

| Category | Leading headlines |
| --- | --- |
| Folding (64 rows, 22 episodes) | 14.0 s median against 5.5 s (δ 0.57). Hands move at 0.17 m/s against 0.25 (δ −0.44). Followed by Pick and Place 52% of the time, by another fold 35%. |
| Smoothing and Spreading (12) | Both hands reach the table: lowest point 3 cm against 10 cm (δ −0.69 right, −0.56 left). Starts 17% into its episode against 44%. |
| Pick and Place (122) | Shorter, 5.5 s against 8.5 s (δ −0.31). Faster hands, 0.26 m/s against 0.20 (δ 0.35). |
| Reaching and Grasping (34) | Shorter, 4.5 s against 7.0 s (δ −0.39). Hands travel 0.70 m against 1.34 m (δ −0.22). |

**One Spiral statement per episode supplies the motion metrics.** Topic names belong to the dataset, so the Break down subtasks node gains a **Motion** setting that lists the arm-state topics to read. It preselects topics whose messages carry a numeric `position` list. Joint metrics read `position`. Hand metrics read the translation of a 16-element row-major `pose` (indices 3, 7, 11) when it is present. On abc130k these are `/left-arm-state` and `/right-arm-state`, whose `RobotState` holds 6 joint positions, velocity, torque, and a 4 × 4 end-effector pose. The statement binds the episode as `$1` and lists that episode's subtask intervals in a `VALUES` table. It numbers samples with `ROW_NUMBER()`, joins each sample to its predecessor for per-sample steps, and aggregates per interval. On the abc130k sample that took 40 statements covering both arms, about 8 s each, and 56 s in total at 8 concurrent. Joints take their names from `JointBinding` when the dataset has one. abc130k has no binding, so its joints read as left 1–6 and right 1–6. Eight of the 40 episodes record an empty `pose`. Hand metrics therefore cover 194 of 263 rows, and joint metrics cover all of them. Each metric row states how many rows it covers; missing values are never treated as zero.

**Spiral gaps found while measuring.** Window functions accept only aggregates and ranking functions, so Spiral has no `LAG`, and the `ROW_NUMBER()` self-join above works around that gap. Before building, decide whether to keep the self-join or add `LAG` to Spiral first. A UUID cannot be written as a literal or cast to text, so one statement cannot name several episodes. Per-episode statements are how the episode endpoint works anyway. `[]` indexing and `power` are unsupported; `spql.list.get` reads list elements with zero-based indices.

**Motion summaries are saved per row, like posters.** The annotation pass computes an episode's motion summaries right after its rows validate, with one more statement for that episode, and saves them on each `SubtaskRow`. Rows annotated earlier are backfilled the first time a profile opens. A summary is about 20 numbers per row. Profiles are derived on the fly and never saved.

### Animation

**Animation shows where rows went and how sure the model was, never decoration.** Every sequence follows the same rules:

- A sequence finishes within about 1.5 s.
- A press anywhere in the workspace completes it instantly.
- `prefers-reduced-motion` gets the final state with no movement.
- Only `transform` and `opacity` animate, apart from the map's colour reveal, which is CSS. The Web Animations API lets the compositor run the frames, so no Rust runs per frame. web-sys gates `Element.animate` behind an unstable cfg, so narrow bindings reach it, as the video decoder already does for WebCodecs.
- FLIP handles both reordering and flights: measure each element before and after the state change, then animate the difference.

One animation primitive serves both explorations: `crates/ui/src/foundation/motion.rs`, with `Arrangement` for FLIP, `Flight` for tokens along a curve, and `Motion` for reduced motion, finishing, and arriving from a remembered place. The spring clock in `containers/reorder.rs` stays for direct manipulation.

**Grouping: the dataset pours into its categories.**

1. Before a run, the map shows every episode's subtasks in grey.
2. The LLM's category proposal is the long wait. During it, a soft highlight sweeps down the map row by row, and the status line counts the distinct descriptions being read.
3. The proposed categories enter one at a time as dashed, empty chips under the map.
4. When jev's assignments land, lanes release top to bottom, spread over 0.7 s. Each segment takes its category's hue in place, and a small token is tossed from it in an arc down into its bin. In about 1.4 s the dataset visibly splits into subtasks and pours into categories. One episode sends pieces to several bins, which makes the point that rows are grouped, not episodes. On abc130k all 263 tokens fly.
5. A row assigned with confidence below 0.9 bends its arc toward its runner-up category before landing, and its map segment keeps a hatched edge. On abc130k, 46 of 263 rows arrive this way, including 8 of the 12 in Smoothing and Spreading, so a soft category looks soft. The flight needs the runner-up category stored with each assignment.
6. The chips re-sort biggest first, sliding into place, and the biggest category opens. The colored map is the resting state, showing the dataset's task structure.
7. Rename, merge, and regenerate reuse the same flight for the diff only: rows whose category changed fly from their segment to their new bin, and rows that stayed put do not move. A merged bin simply disappears.

**Find similar: the operation lights up across the dataset.**

1. Clicking **Find similar** lifts the row's poster and carries it to the query card at the top of the search workspace. On the canvas, the search node is selected, or enters with its edge drawing in.
2. While jev answers, the source segment pulses on the map.
3. When answers land, the list reorders: each row slides from its old place to its rank. Rows out of view before and after move without animating.
4. The map lights each segment by its probability, rippling outward from the source segment, so matches in other episodes appear like echoes. Where the matches sit in their rows shows when in an episode this operation usually happens.
5. A probability strip under the map places every answer as a dot on a 0–1 axis, best first to drop in, with a tick at the 50% line the episode count uses. The gap between the confident cluster and the rest shows how many real matches there are, without a cutoff. Clicking a dot plays that row.

**Status, 2026-09-23:** the episode map, row-level **Find similar**, the table-derived profile, runner-ups, and both animations are built and exercised on the abc130k sample. Not built: the motion pass and its profile rows, the counters that tick as tokens land, and brushing the probability strip.

### Subtask data model
Each dataset gets a subtask table, with one row per subtask interval:

| Column | Meaning |
| --- | --- |
| `episode` | Reference to the source episode |
| `start_time` | Inclusive start, in seconds relative to the episode |
| `end_time` | Exclusive end, in seconds relative to the episode |
| `description` | Generated natural-language description of the subtask |
| `motion` | Not built yet. Per-arm hand travel, lowest hand height, and per-joint range over the interval, from the motion statement; absent until computed |

An episode can have many subtask rows. Search returns rows; the episode reference and time range locate each video slice. Keep the original recordings intact. Search persists no results. Each grouping node persists its categories and assignments:

| Table | Columns |
| --- | --- |
| `subtask_category` | `name`, `definition`, from the LLM pass |
| `subtask_group` | `episode`, `start_time` (the subtask row key), `category`, `probability`, `confidence`, and an optional `runner_up` of `category` and `probability`, from jev; groupings assigned before runner-ups were kept have none |

A Find similar node persists its query text and, when the query came from a row, that row's key.

**Persistence.** The graph, its node configuration, the subtask rows, and the groupings are one versioned JSON document per exploration, saved through `/api/explorations` with the same revision-conflict rule as workflows. The backend stores the payload opaquely; the frontend owns its schema and encoding version. The frontend saves after every annotated episode, so a reload keeps finished rows.

Similarity means the same described operation, as jev judges from the descriptions. The video grid lets the user compare differences in execution. Good boundaries and specific action descriptions are essential; matching text alone does not establish matching motion or success.
