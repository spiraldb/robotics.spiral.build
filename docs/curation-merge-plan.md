# Merging Workflow into Curation

*Plan, 2026-09-24. Phases 0 to 2 are implemented; see [Phasing](#phasing).*

Studio has two ways to shape data for training, and both end in the same
thing: part of a dataset. Workflow is free-form SQL with an Assistant.
Curation is a typed graph whose results live only in Studio. This plan
removes Workflow and makes Curation the one way to choose training data.

You still build a curation interactively on a sample. What it produces is a
**named view in the Spiral catalog**, with **one row per kept interval of an
episode**. For the user, a curation is a SQL query Spiral runs. Every node's
work compiles to Spiral SQL, including model judgments, so the published view
*is* the curation. Making repeated model calls fast and deterministic is
Spiral's job. It is a later optimization, not a precondition.

Curation only **chooses** data: which stretches of which episodes, and what
to call them. How a model *sees* that data (sample rate, cameras, image size,
normalization) is a separate step inside Training. That step is described
below as a direction and is not part of this merge.

## The model

**Curation.** A tree rooted at one Dataset node. Each edge carries episode
intervals with properties. A whole episode is one interval. A subtask is an
interval with a description. A productive stretch is an interval. Every node
is one of two kinds:

| Kind | What it does | Nodes |
| --- | --- | --- |
| Measure | Adds a property to each interval, or cuts intervals into finer ones | Break down subtasks, Find similar, Group similar, Score progress |
| Select | Keeps the intervals whose property passes its rule | Episodes ≥ 10 s; category is *fold*; moving stretches |

**Output.** Every leaf Select is an output. Its relation is published as it
stands: one row per kept interval.

**Two runs of the same SQL.**

- **Preview** runs while you build, on the first ≤100 episodes in recording
  order, one episode per query.
- **Published** runs on the whole dataset. Spiral evaluates it whenever the
  view is read.

**Save and Publish.** Save writes the draft recipe to Studio's local SQLite,
as it does today. Publish creates or replaces a catalog view under a name you
give. A view cannot be turned back into a graph, so the recipe stays in SQLite
and the view is what it produces.

```
Dataset abc130k
└─ Break down subtasks (top camera, every 0.5 s)
   └─ Group similar subtasks (fold, flatten, reach, …)
      └─ Select: category is fold                → Publish as folds
```

## The output view

`folds`:

| episode_id | start_ns | end_ns | subtask | subtask_category |
| --- | --- | --- | --- | --- |
| 01a0…e090 | 1743701335744944896 | 1743701342300000000 | make the first fold | fold |
| 01a0…e090 | 1743701346700000000 | 1743701351200000000 | make the second fold | fold |
| 7c3f…11b2 | … | … | fold the sleeve | fold |

- **`episode_id`**: the dataset member.
- **`start_ns`, `end_ns`**: the kept stretch, in the recording's clock (MCAP
  log time). A reader joins streams without converting times.
- **One column per property** that the path's Measures produced. An
  episode-level output has no property columns beyond what its Selects read.

It has two invariants:

1. **Intervals are ordered by start and may overlap.** Two subtasks can
   happen at once, such as each arm doing something different, so
   `breakdown_task` keeps overlapping subtasks rather than refusing them
   (decided 2026-09-25). A reader that samples moments decides what an
   instant inside two intervals means.
2. **The view holds no stream data.** Cameras and joint states stay in the
   recording at their native rates. Aligning them is the reader's job.

A sketch of `folds`. `breakdown_task` and `classify` are proposed functions,
and elided parts are marked `…`:

```sql
CREATE VIEW folds AS
WITH episodes AS (
  SELECT id AS episode_id, … AS start_ns, … AS end_ns FROM spql.mcap.episodes('abc130k')
),
subtasks AS (
  SELECT e.episode_id, s.start_ns, s.end_ns, s.description AS subtask
  FROM episodes e
  CROSS JOIN LATERAL UNNEST(spql.ai.breakdown_task(<top camera of e>, sample_interval => 0.5)) AS s
),
grouped AS (
  SELECT *, spql.ai.classify(subtask, <'fold' → '…', 'flatten' → '…'>) AS subtask_category
  FROM subtasks
)
SELECT episode_id, start_ns, end_ns, subtask, <category of subtask_category> AS subtask_category
FROM grouped
WHERE <subtask_category is 'fold'>
```

## Decisions

Agreed:

| # | Decision | Why | Rejected |
| --- | --- | --- | --- |
| D1 | Remove Workflow: no SQL node, no joins, no free-form query shapes | Curation always ends in training input. Analytics is not the product. | A SQL escape-hatch node |
| D2 | Every node compiles to Spiral SQL, and a published view is the whole curation | One executable form. Every reader sees exactly what was curated. | Studio materializing results itself |
| D3 | Per-row model work becomes `spql.ai.*` functions (Break down, similarity, classification). Authoring-time LLM calls, such as proposing categories, stay in the browser, and their answer becomes a literal in the node's SQL. | Spiral alone must be able to evaluate the view. An authoring suggestion is configuration, not data. | Browser-orchestrated calls per episode (today) |
| D4 | Caching model results is Spiral's concern: an optimization, done later | Correctness first. Users shouldn't manage cost. | Studio-side caches |
| D5 | A node's preview runs only its own step, over the rows its parent's preview holds, written into the statement as `VALUES`. Editing or re-running a node leaves every node below it out of date, and each re-runs from its parent's current rows. (Revised 2026-09-24; it was: preview recomputes the whole path.) | Each step is still written once and runs the same SQL in both readings, but a search or a grouping makes only its own text calls instead of running Break down again for every episode | Recomputing the whole path per episode, which repeats every model call above the node; a table-parameter endpoint, which does not exist |
| D6 | Preview uses the first ≤100 episodes in recording order | This is the sample today, and it is fast enough to iterate on | A random sample |
| D7 | Outputs are the leaf Selects | Current behaviour. No extra node kind. | An explicit Output node |
| D8 | An output is one row per kept interval | It is what the graph already carries, so publishing converts nothing. Rows stay flat, and each label stays on its own interval. | One row per episode with nested ranges (derivable with `GROUP BY`); one row per moment (rate is a training choice) |
| D9 | Curation only chooses data. Sample rate, cameras, image size, normalization and augmentation belong to Training. | These are the policy's input contract, and rollout must replay it on live frames, so they travel with the checkpoint | Shape nodes in Curation; a separate Shape tab |
| D10 | Publish is explicit and named. Save stays local. | Exploratory leaves must not create catalog resources, and readers need a stable name | Auto-publishing every leaf |
| D11 | No curation Assistant yet. The + menu adds nodes. A jev-style composer inside a node comes later. | Keeps the concept small | A Workflow-style chat column |
| D12 | A published view is one stream of episode rows (decided 2026-09-25). Each per-episode model call is a column of the episode's row, built once where the path leaves whole episodes; its answer rides on the interval rows until the node's own step unnests it, and every step reads the entry above it once. | Spiral pastes a CTE into every place that reads it, so a model step read twice ran twice: moments → Select → Break down ran `score_progress` twice per episode. The cost: only a Select of episodes spares an episode a model call, so that view asks Break down of episodes no stretch of which reaches it. Its rows still equal the preview's. | Each node's CTEs re-reading the one above; SpiralDB computing a CTE read in several places once, not pursued for now |

Proposed here, pending your agreement:

| # | Proposal | Why | Rejected |
| --- | --- | --- | --- |
| P1 | Measures that read frames read the whole episode, and their rows are clipped to the input intervals (agreed 2026-09-25) | Keeps today's behaviour (subtasks keep full-episode context). The model call then depends only on the episode, so editing an upstream threshold never changes what the model is asked, which is good for the later cache. | The model reads only the kept intervals |
| P2 | Interval times are MCAP log time in ns | Readers join streams directly | Times relative to episode start |
| P3 | The frontend compiles. The backend's Publish takes a name and a read-only `SELECT`, admits it with the existing read-only policy, and wraps it in `CREATE VIEW`. | The backend stays thin and provides only DDL. The browser never sends arbitrary DDL. | The backend compiles the graph |
| P4 | Property columns are named by node kind (`subtask`, `subtask_category`, `similarity`), with a suffix on repeats | Stable across renames and readable by readers | Node ids; user labels |

## What each node compiles to

| Node | Configuration | SQL it adds | Spiral support |
| --- | --- | --- | --- |
| Dataset | dataset | Whole-episode intervals and episode facts from `spql.mcap.episodes`; in preview, only the episode bound as `$1` | exists |
| Select | rule on its basis property | `WHERE` on facts, similarity or category. For progress: keep moving moments and merge adjacent ones into stretches (window functions). | exists |
| Score progress | none (scorer pinned) | `spql.ai.score_progress` over the episode's top camera, giving one short interval per scored frame | exists (experimental) |
| Break down subtasks | camera, model, sample interval | `UNNEST` of `spql.ai.breakdown_task` over the episode, clipped to input intervals | **new** |
| Find similar subtasks | query text | `spql.ai.similarity(subtask, query)` | **new** |
| Group similar subtasks | categories (name → definition), proposed in the browser, editable | `spql.ai.classify(subtask, categories)` | **new** |

## Changes to Spiral Robotics

### 1. Remove Workflow

- **Frontend.** Delete `crates/frontend/src/activities/workflow/`, about 7.4k
  lines: agent, tools, plan display, comparison, state, workspace. Delete its
  route in `shell/routes.rs`, its status in `shell/frame.rs`, and its
  `WorkflowSummary` use in `components/document_session.rs`.
- **Backend.** Delete `crates/backend/src/workflow/` (`/workflows`,
  `/workflows/save`, `/workflow-runs`), and in storage `workflows.rs`,
  `workflow_assistant.rs` and the empty `workflow_sql/`. Add a migration that
  drops the `workflows` table, and drop it from project transfer.
- **Common.** Delete `crates/common/src/workflow.rs` and `workflow/` (plan,
  document, comparison, camera).
- **Move, don't delete:**
  - `ProviderMessage` and `ProviderToolCall` go to `common::providers`. They
    are used by `client/completions.rs`, robot discovery and category proposal.
  - `/api/datasets/cameras` goes to the Data routes, and
    `storage/workflows/cameras.rs` moves under `storage/datasets`. Data calls
    it.
- **Docs.** Delete `docs/workflow-authoring.md`. Remove Workflow from
  `PRODUCT.md`, `DESIGN.md` and `docs/http-api.md`.

### 2. The curation document

- **Recipe only:** nodes, edges, configuration, and published outputs
  (`node → view name, published revision`).
- **Results go:** `subtasks`, `groupings`, `rankings`, `moments`, and each
  node's `completed_episodes`, `failed_episodes` and `curves`. Group's
  categories become configuration.
- **New encoding version.** The migration that drops `workflows` also deletes
  every encoding-1 curation; nothing is converted. During phase 1, un-migrated
  nodes keep their results fields until phase 2 removes them.

### 3. The compiler

- Each node kind owns its SQL, next to its configuration type in its feature
  module. There is no `sql.rs` bucket. One path type walks root → node and
  composes the CTEs.
- Literals go through the existing `SqlLiteral` quoting path.
- Each node writes what it asks of each episode's row and its step over the
  interval rows that reach it, each once (D12). Published, the step reads the
  entry above it. Previewed, it reads the parent's preview rows as `VALUES`,
  beside the node's own row for the episode bound as `$1` through
  `/api/queries/episode`; the text judgments batch many episodes in one
  `/api/queries` statement (D5).
- A Select's slider filters the parent's preview rows in the browser. The same
  `Selection` value renders the `WHERE` clause, and a test asserts the two
  agree on the sample.

### 4. The preview runner

- One query per episode for the measures that read a recording, eight in
  flight, and as few batches as fit for the text judgments, through
  `CurationController::start_run`. That gives "12 of 100" progress; Stop
  starts no further query and lets those in flight finish (Spiral cannot
  cancel a running query yet), and a failed episode never stops the rest.
- Rows are held in memory for the session and lost on reload (D3, D5).

### 5. Node changes

- **Break down subtasks:** configuration is unchanged (camera, model,
  interval) and is passed to the function. Delete the browser pipeline:
  `annotate/pipeline.rs`, `request.rs`, `sheet.rs`, `provider.rs`,
  `journal.rs`, `native.rs`. Keep the workspace (`output.rs`, `episodes.rs`).
- **Find similar / Group similar:** delete the jev question builders
  (`similar/questions.rs`, `similar/decisions.rs`) and `client/decisions.rs`.
  Category proposal (`similar/categories.rs`) stays in the browser through
  `ModelClient`.
- **Score progress:** compiles to `spql.ai.score_progress`.
  Delete `/api/curations/progress-score` and
  `backend/src/curation/episode_progress.rs`.

### 6. Publish

- **Leaf Select workspace:** Publish, then a name prompt, then the backend
  creates the view, then the document records it and saves. With unsaved
  edits, Publish asks you to save first. The node shows when the saved recipe
  has moved past the published revision. Unpublish drops the view.
- **Backend `POST /api/curations/publish {name, statement}`:**
  - validates the identifier;
  - admits the statement with `ReadOnlyQuery`;
  - refuses a name held by a table or Dataset;
  - runs one `CREATE OR REPLACE VIEW`. (Implemented so: `DROP VIEW IF EXISTS`
    then `CREATE VIEW` in one transaction fails with a namespace
    compare-and-swap conflict once the view exists.)

### 7. Docs

Rewrite `docs/curation.md` around this model. It still describes the deleted
rule recipe.

## Changes to SpiralDB

### Required for this merge

1. **`spql.ai.breakdown_task`: split one episode's camera video into subtasks.**
   - Input: one camera's packet video (the type `score_progress` takes), a
     sampling interval, and a model id with a pinned revision.
   - Output: `list<struct{start_ns, end_ns, description}>` ordered by start,
     possibly overlapping, on the video's presentation timeline.
   - For now it calls a remote multimodal LLM through OpenRouter. Spiral takes
     over the prompt and contact-sheet composition that Studio does today;
     `annotate/request.rs` and `annotate/sheet.rs` are the reference.
   - Later it runs a local model with the same signature.
   - Use low reasoning effort. On a 20 s abc130k episode it measured $0.0066
     against $0.059 at medium, with comparable subtasks.
2. **Text judgments, named by what they answer rather than by model:**
   - `spql.ai.similarity(text, query) → double in [0, 1]`
   - `spql.ai.classify(text, categories) → struct{category, probability}`
   - Implementing them with jev (OpenRouter Decisions) or with text embeddings
     is Spiral's choice. Today `spql.ai.embed` has only an image overload.
3. **Contract for model functions:**
   - A failed row returns NULL, with its reason readable. It never fails the
     query: a 130k-episode view cannot die on one episode.
   - Credentials come from Spiral's credential store, with the OpenRouter key
     beside the Hugging Face token.
   - Model and revision are arguments, so every result is attributable (and
     cacheable later).
   - Calls are evaluated in parallel, within provider rate limits and with
     retries.
4. **Views:**
   - `CREATE VIEW` over Datasets and `external_io` functions. Root-level
     views already work (`dataset_source_<name>`).
   - Atomic replace: `CREATE OR REPLACE VIEW`, or drop and create in one
     transaction.

### Later: optimizations

5. **Caching model results, so re-reads are fast and deterministic.** Two
   designs:

   | | Materialized view with optimizer rewrite | Per-call cache |
   | --- | --- | --- |
   | Unit | A view or subexpression | One call: function, model revision, inputs |
   | Reuse | Queries matching the materialized plan | Any query making the same call |
   | Invalidation | Refresh when the source changes | Content-addressed, so nothing to invalidate |
   | Visibility | An explicit catalog object | Invisible |

   Studio's access pattern favours the per-call cache. Preview and the
   published view make the same calls on the same episodes (P1), and so do two
   curations over one dataset. A per-call cache shares that work without either
   side knowing.

   Until caching lands, every read of a published view repeats its model
   calls. That is slow, costs about $0.0066 per 20 s episode for Break down at
   low effort, and remote LLM answers can differ between reads.
6. **Query cancellation through ADBC,** so Stop can end a running preview
   query instead of waiting for the next episode.

## Training: the direction, not part of this merge

Training stays as it is during the merge and does not read curation outputs
yet. This section records where it should go, so the curation output is
designed for it.

**Training resamples.** A recording keeps every stream at its own rate, and a
curation output only names intervals. Training picks a sample rate and
produces one moment per tick inside each interval: the value of each chosen
stream at that tick, plus the interval's properties. That rate is also the
rate rollout must run at, because a policy learns the time between the actions
of a chunk. This matches common practice:

- **Same rate at training and rollout:**
  - [LeRobot](https://raw.githubusercontent.com/huggingface/lerobot/main/src/lerobot/scripts/lerobot_record.py)
    runs its control loop at the dataset's fps and refuses a different one.
  - [ACT](https://ar5iv.labs.arxiv.org/html/2304.13705) records and controls
    at 50 Hz.
  - [π0](https://arxiv.org/html/2410.24164) runs each robot at its data's rate
    (20 Hz or 50 Hz).
  - [Open X-Embodiment](https://arxiv.org/html/2310.08864v4) runs each model
    "at the rate required for the robot" (3–10 Hz).
- **Resampling inside the training pipeline:**
  [Diffusion Policy](https://arxiv.org/html/2303.04137v5) predicts at 10 Hz
  and interpolates to the 125 Hz controller.
  [UMI](https://arxiv.org/html/2402.10329) downsamples cameras to 10–20 Hz and
  interpolates proprioception at each frame's capture time.
- **Mixed rates:** models trained on mixed rates take the rate as an input,
  like [RDT-1B](https://arxiv.org/html/2410.07864).

**The checkpoint carries how the model sees data, and rollout replays it.**
This covers the sample rate, which streams are observations and which is the
action, image size, normalization, and how each stream fills a tick (nearest
frame, latest value, or interpolation). openpi
([`policy_config.py`](https://raw.githubusercontent.com/Physical-Intelligence/openpi/main/src/openpi/policies/policy_config.py))
builds inference preprocessing from the training config and the checkpoint's
normalization statistics. LeRobot saves its
[processor pipelines](https://huggingface.co/docs/lerobot/introduction_processors)
with the policy. Our checkpoint already carries `sample_hz`, camera streams,
image normalization and frame alignment, and rollout checks them.

**Each image adjustment has one home:**

| Adjustment | Home |
| --- | --- |
| Colour space when the recording lacks metadata | The dataset: a fact about the recording, set once in Data and used by every reader |
| Exposure normalization applied at training and rollout | Training's input contract |
| Random brightness or colour jitter | Training augmentation, never replayed at rollout |
| A few badly exposed episodes | Curation: measure brightness and exclude them |

**Changes, when this is picked up:**

- **Config.** Replace `dataset_id`, `sql`, `state_column` and `action_column`
  with a published curation view, observation streams and an action stream.
  Add a sample rate and a fill rule per stream. `camera_streams`,
  `image_width` and `image_height` already live in Training and stay.
- **Picker.** Lists catalog relations that have `episode_id`, `start_ns` and
  `end_ns`.
- **Near an interval's end.** Whether action chunks and history read the
  recording past the interval's edges or are padded becomes a training choice,
  since the resampler can read the whole episode.
- **Run record.** Records the view definition the run read.
- **Runtime.** `McapTrainingDataset` becomes the resampler.

**SpiralDB needs, for that phase:**

- Every frame of a camera within an interval as `(time_ns, image)` rows,
  decoded sequentially. Today `frame_at` or `frame_indices` plus `decode`
  starts from the keyframe for every frame.
- Reliable per-frame presentation times. `frame_times` returns nullable
  durations today.
- Decoded fields of state and command topics as numeric columns keyed by log
  time. `ASOF JOIN` already works (checked on the dev project).
- An exposure adjustment in `spql.image`, which today can only measure it
  (`mean_luminance`).

## Phasing

| Phase | Spiral Robotics | SpiralDB | Result | Status |
| --- | --- | --- | --- | --- |
| 0 | Remove Workflow | none | One authoring surface | Done |
| 1 | Compiler, per-episode preview and Publish, for Dataset, Select and Moments | Item 4 | Curations without LLM nodes publish. Break down, Find similar and Group work as today, but a path through them cannot publish and names the function it waits for. | Done |
| 2 | Break down, Find similar and Group similar compile. Delete the browser pipelines, the jev client and stored results. | Items 1–3 | Every node publishes | Done, 2026-09-24. Studio passes `LLM_API_KEY` as `spiral.openrouter_api_key`, and previews read their parent's rows (D5). |
| 3 | none | Items 5–6 | Fast, deterministic re-reads | |
| Later | Training resamples a published curation (see above) | Its SpiralDB needs | Train on curated data | |

## Open questions

1. **Published view names.** Plain names at the catalog root, or a prefix such
   as `curated_`? Arbor has no child views, and the catalog does not infer
   ownership from names.
2. **Dataset refreshes.** A view is live, so recordings added to its dataset
   appear in it. Should a training run pin its data, or accept the view as it
   reads it? This can wait for the Training phase.
3. **P2–P4.** Agree or amend. (P1 was agreed on 2026-09-25.)
