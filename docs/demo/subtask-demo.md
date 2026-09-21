# Demo: Find the same subtask across episodes

### Overall flow
Turn a robot dataset into searchable moments: inspect one demonstration, discover its subtasks, and find the same operation across other episodes.

1. **Inspect an episode.** Open a cloth-folding demonstration and play its camera and robot-state views.
2. **Break it into subtasks.** A vision-language model reads sampled frames and produces timestamped descriptions: grasp the corners, make the first fold, smooth the cloth. Show these as clickable timeline segments.
3. **Annotate every episode.** Run the same process across the dataset and save each episode's subtask intervals.
4. **Find similar subtasks.** Select “make the first fold” and search for related operations across the dataset using embeddings of the descriptions.
5. **Compare the results.** Show a grid of matching video slices. Each plays only its subtask interval and identifies the source episode and time range. Clicking a result returns to its original context.

### Proposed UI

**Build** adds a dataset → measurement → exploration graph to Workflow, alongside the existing **Agent** tab.

Start with one dataset node. A small **+** centered on its bottom edge opens a measurement menu: **Break down subtasks** or **Find unproductive moments**. Choosing one creates a connected node. Use the same quiet bottom-center **+** on measurement nodes for their supported child actions; no labeled button row.

**Chosen layout: [B — graph beside results](../../.impeccable/mocks/workflow-build-b.png).** Keep the graph in a resizable left pane and a larger workspace for the selected node on the right. Match the current Studio design: compact tabs and controls, white panels, pale dotted canvas, thin borders, and blue selection. The mockup's imagery and counts are illustrative.

Latest interaction mockups: [search results](../../.impeccable/mocks/workflow-build-b-v2.png) and [subtask output with the + menu open](../../.impeccable/mocks/workflow-build-subtasks-v2.png).

Selecting a node changes the right workspace:

- **Dataset:** browse and inspect episodes.
- **Break down subtasks:** configure and run annotation, then inspect the output table and annotated timeline. The node summarizes its run state and output; the table uses the schema below.
- **Find similar subtasks:** a child node with a search box and a grid of matching camera slices. Each result shows its description, source episode, and interval.
- **Group similar subtasks:** a child node showing named groups with representative slices; selecting a group reveals its members. Group subtask rows, so an episode may appear in several groups. Clustering method remains TODO; k-means is a candidate.

Each measurement's **+** exposes only the child actions its output supports. Search and grouping reuse the saved subtask table. Changing a search does not rerun annotation. Keep video results in the workspace so graph nodes remain small and readable.

**Implementation order:** implement and persist the Build graph first, clearly marking measurements and explorations that cannot execute yet. Add annotation and embedding search afterward.

### Annotation

**Orchestration lives on the frontend, following the Workflow agent.** A Build measurement controller owns episode iteration, contact-sheet preparation, prompts, model/embedding requests, result validation, and progress. It uses backend APIs for native frame sampling and persistence. For model calls, the backend holds credentials, forwards requests to the provider, and handles streaming, transport retries, and cancellation; it does not own the annotation loop. This measurement is a fixed pipeline with one annotation call per episode. Closing or reloading the browser stops its orchestration; already-persisted rows remain available.

**One annotation call per episode.** Start with one selected camera and **Gemini Flash** as the model candidate. Our proposed pipeline adapts [Macrodata's frame-packing and single-pass approach](https://macrodata.co/blog/annotating-robot-video-subtasks):

1. Select frames every 0.5 seconds using the recording's timestamps. Resize to 224px-wide tiles, draw episode-relative timestamps, and arrange 20 frames into each 4 × 5 image grid.
2. Send **all grids for that episode in chronological order in one request**, with the task instruction and a structured output schema. Ask for meaningful manipulation events, returning `{start_time, end_time, description}` for each. The model infers durations from changes in the action; there is no fixed subtask length.
3. Validate the returned intervals in code: nonempty descriptions, finite times, `0 ≤ start_time < end_time ≤ episode_duration`, chronological order, and no overlaps for this initial single-sequence annotation. Report invalid output before saving.
4. Generate description embeddings separately and persist the episode's subtask rows. Search reuses those rows without rerunning video annotation.

**Grid boundaries do not split tasks.** Each grid packages roughly 10 seconds of samples; the model's context is the whole episode. For example, a fold starting at 8 seconds and finishing at 13 seconds spans two grids but produces one `[8, 13)` row. Keep timestamps continuous across grids and explicitly tell the model that sheet edges are not action boundaries. Sampling at 0.5 seconds gives approximate temporal evidence, not frame-accurate boundaries.

Macrodata found that separate calls per grid introduced artificial cuts, even with overlap. Its optional second pass relabels predicted segments with neighboring context; we defer that until the single-pass labels prove insufficient. Very long episodes that exceed request limits need a separate chunking/reconciliation design, outside the initial demo.

**Reuse existing components; add the annotation-specific pieces:**

| Existing component | Planned reuse |
| --- | --- |
| Dataset/episode selection and native Spiral video sampling | Read the selected recording and decode sampled frames; adapt the existing sampling helpers to time-based selection. |
| Browser-owned Workflow controller and shared `ModelClient` | Reuse the task lifecycle and project-scoped model transport; the measurement owns its own progress and requests, independently of the chat conversation. |
| `ModelRequest`, multimodal messages, and backend provider proxy | Build annotation prompts on the frontend and keep credentials on the backend. Add structured output and Gemini configuration; current transport is OpenAI-compatible, so Gemini support must be verified, not assumed. |
| `EpisodeCamera`, `EpisodeTransport`, shared `Timeline` | Reuse playback, seeking, and interval playback; add a labeled subtask track. Extract Curation's `EpisodeReview` composition for sharing instead of building another player. |

New work remains: the frontend measurement controller, contact-sheet assembly, the annotation prompt/schema and validator, embedding transport, and subtask-table persistence. These are proposed additions, not capabilities already implemented in the repo.

### Subtask data model
Each dataset gets a subtask table, with one row per subtask interval:

| Column | Meaning |
| --- | --- |
| `episode` | Reference to the source episode |
| `start_time` | Inclusive start, in seconds relative to the episode |
| `end_time` | Exclusive end, in seconds relative to the episode |
| `description` | Generated natural-language description of the subtask |
| `desc_embedding` | Text embedding of the description, used for similarity search |

An episode can have many subtask rows. Search returns rows; the episode reference and time range locate each video slice. Keep the original recordings intact.

Similarity initially means the same described operation. The video grid lets the user compare differences in execution. Good boundaries and specific action descriptions are essential; matching text alone does not establish matching motion or success.
