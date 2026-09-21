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

**Implementation order:** implement and persist the Build graph first, clearly marking measurements and explorations that cannot execute yet. Add annotation and embedding-search services afterward.

### Annotation
Follow [Macrodata's approach](https://macrodata.co/blog/annotating-robot-video-subtasks): sample frames every 0.5 seconds, resize to 224px-wide tiles, and pack 20 timestamped frames per image grid. Send all grids for an episode together and ask for meaningful action boundaries with free-form descriptions. Start with a single pass; **Gemini Flash is the initial model candidate**. Generate description embeddings separately for search.

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
