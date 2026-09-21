# Demo: Find the same subtask across episodes

### Overall flow
Turn a robot dataset into searchable moments: inspect one demonstration, discover its subtasks, and find the same operation across other episodes.

1. **Inspect an episode.** Open a cloth-folding demonstration and play its camera and robot-state views.
2. **Break it into subtasks.** A vision-language model reads sampled frames and produces timestamped descriptions: grasp the corners, make the first fold, smooth the cloth. Show these as clickable timeline segments.
3. **Annotate every episode.** Run the same process across the dataset and save each episode's subtask intervals.
4. **Find similar subtasks.** Select “make the first fold” and rank every other subtask in the dataset by how likely it describes the same operation. Ranking runs on the fly with jev, the decision model Curation already uses; nothing is precomputed.
5. **Compare the results.** Show the ranked grid of video slices, best match first. Each plays only its subtask interval and identifies the source episode and time range. Clicking a result returns to its original context.

### Proposed UI

**Build** adds a dataset → measurement → exploration graph to Workflow, alongside the existing **Agent** tab.

Start with one dataset node. A small **+** centered on its bottom edge opens a measurement menu: **Break down subtasks** or **Find unproductive moments**. Choosing one creates a connected node. Use the same quiet bottom-center **+** on measurement nodes for their supported child actions; no labeled button row.

**Chosen layout: [B — graph beside results](../../.impeccable/mocks/workflow-build-b.png).** Keep the graph in a resizable left pane and a larger workspace for the selected node on the right. Match the current Studio design: compact tabs and controls, white panels, pale dotted canvas, thin borders, and blue selection. The mockup's imagery and counts are illustrative.

Latest interaction mockups: [search results](../../.impeccable/mocks/workflow-build-b-v2.png) and [subtask output with the + menu open](../../.impeccable/mocks/workflow-build-subtasks-v2.png).

Selecting a node changes the right workspace:

- **Dataset:** browse and inspect episodes.
- **Break down subtasks:** configure and run annotation, then inspect the output table and annotated timeline. The node summarizes its run state and output; the table uses the schema below.
- **Find similar subtasks:** a child node with a search box and a ranked grid of camera slices, best match first. The query is the selected subtask's description or typed text. Every subtask row appears in the ranking with its jev probability; there is no cutoff. Each result shows its description, source episode, interval, and probability.
- **Group similar subtasks:** a child node showing 4–8 named categories with representative slices and member counts; selecting a category lists its members, most typical first. The categories come from one LLM pass over the concatenated descriptions; membership comes from one jev Choice per subtask row. Rows are grouped, not episodes, so an episode may appear in several categories.

Each measurement's **+** exposes only the child actions its output supports. Search and grouping read the saved subtask table and never rerun annotation. Search persists nothing; grouping persists its categories and assignments. Keep video results in the workspace so graph nodes remain small and readable.

**Implementation order:** implement and persist the Build graph first, clearly marking measurements and explorations that cannot execute yet. Add annotation, then jev search and grouping.

### Annotation

**Orchestration lives on the frontend, following the Workflow agent.** A Build measurement controller owns episode iteration, contact-sheet preparation, prompts, model and jev requests, result validation, and progress. Contact sheets are drawn in the browser from the Data view's episode decoder; there is no backend frame-sampling API. The backend stores one versioned Build document per graph and never interprets it. The frontend reads Studio-wide credentials, constructs authenticated provider requests, and handles provider responses and retries. The backend relays HTTP through `/api/proxy`, preserving streaming and cancellation without interpreting model requests or selecting credentials. This measurement is a fixed pipeline with one annotation call per episode. Closing or reloading the browser stops its orchestration; already-persisted rows remain available.

**One annotation call per episode.** Start with one selected camera and **Gemini Flash** as the model candidate. Our proposed pipeline adapts [Macrodata's frame-packing and single-pass approach](https://macrodata.co/blog/annotating-robot-video-subtasks):

1. Select frames every 0.5 seconds using the recording's timestamps. Resize to 224px-wide tiles, draw episode-relative timestamps, and arrange 20 frames into each 4 × 5 image grid.
2. Send **all grids for that episode in chronological order in one request**, with the task instruction and a structured output schema. Ask for meaningful manipulation events, returning `{start_time, end_time, description}` for each. The model infers durations from changes in the action; there is no fixed subtask length.
3. Validate the returned intervals in code: nonempty descriptions, finite times, `0 ≤ start_time < end_time ≤ episode_duration`, chronological order, and no overlaps for this initial single-sequence annotation. Report invalid output before saving.
4. Persist the episode's subtask rows. Search and grouping read those rows later; neither reruns video annotation.

**Grid boundaries do not split tasks.** Each grid packages roughly 10 seconds of samples; the model's context is the whole episode. For example, a fold starting at 8 seconds and finishing at 13 seconds spans two grids but produces one `[8, 13)` row. Keep timestamps continuous across grids and explicitly tell the model that sheet edges are not action boundaries. Sampling at 0.5 seconds gives approximate temporal evidence, not frame-accurate boundaries.

Macrodata found that separate calls per grid introduced artificial cuts, even with overlap. Its optional second pass relabels predicted segments with neighboring context; we defer that until the single-pass labels prove insufficient. Very long episodes that exceed request limits need a separate chunking/reconciliation design, outside the initial demo.

**Reuse existing components; add the annotation-specific pieces:**

| Existing component | Planned reuse |
| --- | --- |
| Dataset/episode selection and the browser episode decoder | Open the selected recording, decode frames at 0.5-second timestamps in the browser, and draw contact sheets on a canvas. |
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

Nothing is persisted; a new query is one more round of requests. Sizing: a 2,000-row table at about 400 candidates per request is 5 concurrent requests of roughly 30k tokens each, well under a cent, with sub-second latency per request. The alternative in TypeSafe's [line-by-line search cookbook](https://docs.typesafe.ai/cookbooks/semantic_find.md), one Choice over all candidates, caps at 255 options and returns probabilities that sum to 1 per request, so batches would not be comparable. Prefer the Noul per pair. The cookbook sends one request per pair; if batched Nouls rank measurably worse, shrink the batches.

**Group similar subtasks: the LLM proposes categories, jev assigns rows.**

1. Concatenate the distinct subtask descriptions (dedupe exact text, keep counts) into one request to the annotation LLM. Ask for 4–8 categories with structured output: a short name and a one-sentence definition that distinguishes it from the other categories. Validate in code: 4–8 entries, unique names, nonempty definitions.
2. Assign each subtask row with one jev Choice. State: `{"categories": {...}, "subtasks": {"r0": "...", ...}}`. Instructions per row: “Which category does `subtasks.r0` belong to?” Criteria: the category names mapped to their definitions. Batch rows per request as in search.
3. Every row goes to its highest-probability category. There is no “other” option and no confidence gate: the categories were derived from these same descriptions, so the list is complete by construction. Keep each answer's probability and confidence to order members, most typical first, and to let the user spot ambiguous rows.
4. Persist the category set and the per-row assignment. Renaming, merging, or editing a category definition reruns only the jev assignment, which is cheap. Regenerating categories reruns the LLM pass.

### Subtask data model
Each dataset gets a subtask table, with one row per subtask interval:

| Column | Meaning |
| --- | --- |
| `episode` | Reference to the source episode |
| `start_time` | Inclusive start, in seconds relative to the episode |
| `end_time` | Exclusive end, in seconds relative to the episode |
| `description` | Generated natural-language description of the subtask |

An episode can have many subtask rows. Search returns rows; the episode reference and time range locate each video slice. Keep the original recordings intact. Search persists nothing. Each grouping node persists its categories and assignments:

| Table | Columns |
| --- | --- |
| `subtask_category` | `name`, `definition`, from the LLM pass |
| `subtask_group` | `episode`, `start_time` (the subtask row key), `category`, `probability`, `confidence`, from jev |

**Persistence.** The graph, its node configuration, the subtask rows, and the groupings are one versioned JSON document per Build graph, saved through `/api/build-documents` with the same revision-conflict rule as workflows. The backend stores the payload opaquely; the frontend owns its schema and encoding version. The frontend saves after every annotated episode, so a reload keeps finished rows.

Similarity means the same described operation, as jev judges from the descriptions. The video grid lets the user compare differences in execution. Good boundaries and specific action descriptions are essential; matching text alone does not establish matching motion or success.
