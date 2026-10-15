# Curation

Curation chooses the data to train on: which stretches of which episodes. A
curation is a graph rooted at one dataset. Model nodes add what a model
answered about each episode as columns of one table, and Select nodes keep
part of it by conditions on those columns. You build it against a preview of
the dataset's first 100 episodes, and you publish any node as a named view: a
read-only query Studio keeps and Spiral runs whenever it is read, with one
row per episode over every episode of the dataset. Every model answer is
stored: publishing asks each model about the episodes that reach its node
once, in the background, and appends the answers to the dataset's answer
columns, which the view reads without asking any model again.

How a model sees that data (sample rate, cameras, image size, normalization)
is Training's concern, not Curation's. [Why](#why) records the decisions
behind this model.

## The table

Every edge carries one table with **one row per episode**, never more
(upstream SpiralDB is moving to one episode as one row, and Curation follows
it). The Dataset node starts it: `episode_id`, the facts the dataset's
format records for each episode (`duration_seconds`, `messages`, and for
MCAP recordings `channels` and `size_megabytes`; a LeRobot episode has no file
of its own), and `kept`, the episode's kept time ranges as a list of
`{start_ns, end_ns}`, starting as the whole episode. Times are the
recording's log time in nanoseconds everywhere in the table (a LeRobot
episode's count from its start), so a reader can join the recording's streams
without converting them. The document keeps the dataset with its format
(`dataset: {id, format}`, `format` `mcap` or `lerobot`), as the catalog lists
it when the dataset is chosen.

**Model nodes only add columns.** They never drop a row or narrow `kept`, and
they have no rule. Each reads whole episodes, and only the episodes that
reach it: a row a Select dropped above is never sent to a model. A node adds
one column per thing its settings name; what varies per episode, such as how
many objects a picture holds, stays inside the cell. Timelines hold one
`timestamp_ns` per entry, and an entry holds until the next one.

| Node | Settings | Adds |
| --- | --- | --- |
| Track objects | Track [prompt] on [camera] at [rate] pictures a second (1 to 30, default 5) | One timeline: `List<{timestamp_ns, objects: List<{id, score, x, y, width, height}>}>`, by sampled picture, each object's box as fractions of the picture from its top left |
| Score progress | the model it scores with: the released head, or a published progress model by name ([Training](training.md#published-models)) | One timeline: `List<{timestamp_ns, progress_rate}>`; a frame the scorer had no support for has no rate |
| Break down subtasks | camera, sample interval, where the episode's task is read | `List<{description, start_ns, end_ns}>`, the subtasks of the whole episode |
| Find similar subtasks | query | `List<{start_ns, end_ns, similarity}>`: every subtask of its Break down with how likely it is what the query names |
| Group similar subtasks | categories | One column per category: `List<{start_ns, end_ns}>` of the subtasks it put there |

Break down's subtasks cover each episode end to end: each starts where the
previous one ends, with no gaps and no overlaps, so reaching, pausing,
handing over, and retracting are subtasks too, and two arms acting at once
share one subtask. Find similar and Group similar judge the subtasks of the
nearest Break down above them, which they need. A category column is named
by its node and the category's stable id (`group_similar_subtasks_4_category_2`),
never by its name, which is only its label: renaming a category leaves every
Select reading it intact.

Score progress scores with the model its bar names ("Score with"): the
released head, or a progress model published from Training, listed by name
and never by id. Its document keeps the published model's string
(`spiral-robotics-models/<sha256>`) and the camera it was trained on, not the
name, and the released head when it names none, so older documents decode
unchanged. Each model and camera is its own stored column, so scores of two
models never mix and a model's scores are reused wherever it is chosen again.
A document naming a model this project has not published shows why and
cannot run. The Modal scorer loads a published head from the models Volume
the first time it is asked for it; publishing needs no redeploy.

Column names are the compiler's, not found by inspecting types: each node's
id names its columns (`track_objects-1` writes `track_objects_1`), and
`CurationPath` keeps which node wrote each column and what it means
(`TableColumns` in `path/table.rs`), so a Select, Data, and Training read a column
by the node that wrote it.

## Select

A Select is the one filter, and may follow any node. It holds conditions,
and an episode is kept when all of them hold, each judged over the episodes
that reach the Select. Its menu of conditions is built from everything
upstream: the dataset's facts, and what SQL can derive from each column a
node above added (`CurationGraph::conditions_for`).

**Episode conditions** keep or drop the row: a number per episode compared
*at least*, *at most*, *lowest N%*, or *highest N%*. The numbers are the
facts the dataset records and, for each Track objects node above, the most of what it tracks one
picture held ("most bottles at once") and the episode's duration per that
count ("duration per bottle"). An episode without the number fails the
condition. *Lowest N%* keeps an episode whose percent rank, among the
reaching episodes that have the number, is at most N%: its rank among them,
from 0, over their count less one (`PERCENT_RANK`, which Spiral spells with
`RANK` and `COUNT` windows); *highest N%* ranks from the top.

**Range conditions** narrow `kept`, intersected with what is already kept:

| Condition | On | Keeps |
| --- | --- | --- |
| Progress rate at least x | a Score progress timeline | each frame's moment, from its time to the next frame's (the last lasts as long as the one before it), whose rate is at least x or which has no rate, joined into stretches |
| At least n objects visible | a Track objects timeline | each picture's moment holding at least n objects, joined into stretches |
| Similarity at least x | a Find similar column | the ranges of the subtasks at least that similar |
| In categories | a Group similar node | the ranges of the chosen categories' columns |

Kept ranges that overlap or touch are merged into one, so two subtasks back
to back reach Training as one stretch. A row whose `kept` becomes empty is
dropped. The Select's workspace is where
every cutoff is reviewed: each condition's strip with its slider over the
values it judges, the Kept and Left out tallies, the kept and left-out
stretches replayed beside their recording, and a grouping's categories as
checkboxes.

## Compiling a path

`CurationPath` in `path.rs` walks from the dataset down to one node and
compiles it to one statement. The table is built in two halves that never
read each other's rows:

- **The reach chain**: one row per episode reaching each node, with its
  `kept` ranges, and nothing else. The Dataset starts it, a model node hands
  on its parent's, and each Select narrows it. Each Select reads the entry
  above it once.
- **The columns**: every model node folds its stored answer into its columns
  per episode, read from the episode row, never from the chain. The final
  `SELECT` joins the reach chain to the facts and every column of the nodes
  on the path.

| Node | What it writes | Where |
| --- | --- | --- |
| Dataset | `dataset`: every episode of `datasets.<id>.episodes` with its facts and `kept` of one range, through the listing `Episode` uses everywhere | `graph/dataset.rs` |
| Select | its conditions: `select_N_numbers` and `_ranked` for episode conditions, `_within_K` the ranges a range condition keeps, `_kept_K` the intersection, `_islands` merging ranges that overlap or touch, then `select_N` folding them back into `kept` | `select/step.rs` |
| Score progress | entries: the scores unnested, one per frame, with its log time and progress rate (`CASE WHEN support > 0 AND NOT isnan(velocity)`) | `score_progress/scoring.rs` |
| Track objects | entries: the tracks unnested, one per picture, with its log time and objects | `track_objects/tracking.rs` |
| Break down subtasks | entries: the subtasks listed with their place in the answer (`subtask_index`), moved from the video's timeline to log time | `annotate/breakdown.rs` |
| Find similar, Group similar | entries: the subtasks of the Break down above, each with its judgment read at its `subtask_index` in the stored list | `similar/judgment.rs` |

A model node's entries are one row per thing it found in an episode
(`{node}_entries`), and its column entry (`{node}`) folds them back into one
list per episode with `spql.list.collect`, over the entries ordered first
(Spiral has no ordered `ARRAY_AGG`, and `collect` keeps its input's order). A
grouping folds one list per category with `FILTER (WHERE category = …)`. A
Select reads the entries it needs: a timeline's to find stretches, a judgment's
to keep subtasks, Track objects' to count. The published statement:

```text
dataset                     every episode: facts, and kept as its whole span
episodes                    one row per episode: origin_ns and each stored answer the path reads
track_objects_1_entries, …  each model node's entries over the episode row
track_objects_1, …          each model node's columns, one row per episode
select_2_numbers, …         each Select over the reach entry above it, ending in select_2 (episode_id, kept)
SELECT episode_id, kept, facts, columns   the last reach entry, joined to the facts and the columns
```

Stretches are found by the difference of two `ROW_NUMBER`s and measured with
`MIN` and `MAX` grouped per stretch. A moment's end is `LEAD`, and the last
lasts as long as the one before it (`LAG`); a single frame has no moment.
Spiral reads a field of a column, not of a function's result, so a listed
subtask is named before its fields are read.

### Stored answers

Every model call a node makes is one answer column, named for what it
measures and six hex digits of a SHA-256 of the exact call
(`score_progress_3f9a2c`): the function or service, the model revision, the
Modal service revision, the arguments (Track objects' rate among them), the
camera or where the task is read, and for a judgment the Break down column it
reads. Identical calls across curations share a column. Each column is a
table of its own beside the dataset's `episodes`,
`datasets.<id>.answers.<column>` ([datasets](datasets.md#catalog-layout)):
`episode_id` and `value`, one row per episode asked. A row whose `value` is
NULL is an episode the model could not answer, which is never asked again,
and an episode with no row is not asked yet, as every episode is when it is
added: writing episodes never touches the answer columns. Fills only append
rows, never update or delete one, so the columns need no SQL writes. Each
node's table column is derived from its stored answer in SQL, so answers
stored before a column's shape changed are read again without asking any
model. Unpublishing drops only the view.

A column's table exists once a fill run has started filling it: the run
makes it, empty, before it asks about any episode, its `value` typed as the
call's answers are (a model's as its call declares them, a dataset
statement's as it plans with `LIMIT 0`) with every nested field nullable. So
every statement a curation compiles finds the tables it reads: a published
view is created only once its run has filled every column it reads, a
preview reads only after its fill, and the episodes reaching a node are read
over the columns filled before it, even ones no episode reached. The episode
row joins each column's table with a `LEFT JOIN` on `episode_id`, so an
episode not asked yet and one whose answer failed both read NULL. A reader
that may come before any run asks first instead of reading a table that may
not be there: a curation opened asks which episodes a column holds (none
before its table exists), and the dataset health check asks which answer
columns the dataset holds (`POST /api/curation-answers/columns`, read from
the catalog) and reads only those.

| Node | Stored answer, per episode |
| --- | --- |
| Score progress | the Modal service's WARP scores, by the chosen model, of the camera that model was trained on (the released head's: the top camera): `{timestamp_ns, frame_index, velocity, support, …}` per frame, from the episode's first message |
| Track objects | the Modal object tracking service's SAM 3.1 tracks at the node's rate: `{timestamp_ns, objects}` per sampled picture, from the episode's first message |
| Break down | the subtasks `google/gemini-3.8-flash` reads off contact sheets of the chosen camera's video, told the episode's task when the dataset records one — its first `/instruction` message, or the first of the `tasks` a LeRobot dataset lists (`EpisodeTask`): `{start_ns, end_ns, description}` on the video's timeline |
| Find similar | the similarity to the query `typesafe/jev-1.13` judges for every subtask of the Break down above, one list of doubles aligned with its subtasks |
| Group similar | the category `typesafe/jev-1.13` judges most probable for every subtask, the same way, the categories carried in the call: `{category, probability}` |

`CurationPath::fills` compiles, for each node that asks a model in path
order, its column, the episodes that reach it (the reach chain above it,
compiled over the columns already filled, `SELECT episode_id`), and how one
episode's answer is computed (`AnswerSource`): for Score progress and Track
objects their Modal service, and for Break down and the judgments of its
subtasks a call the backend makes itself through OpenRouter, named by what
it asks — the camera, the sample interval, and where the task is read; the
Break down column and the query or the categories — and the model. A node
names its camera and instruction by topic, which the backend finds among the
streams the dataset records (`DatasetStreams`). Break down's subtasks are
listed in the view with `spql.range` and `spql.list.get`, each carrying its
place, and a judgment reads each subtask's answer at that place; Spiral has
no `WITH ORDINALITY`.

Break down (`crates/backend/src/curation/break_down.rs`) reads the
episode's camera packets in decode order (`CameraVideo::packets`) and its
task: the first message on the instruction topic, or the first task its
LeRobot listing lists. It samples a picture every sample interval from the
first, each the nearest picture and none twice, over a timeline that runs
one mean picture period past the last. FFmpeg (`SPIRAL_ROBOTICS_FFMPEG`, or
`ffmpeg` on `PATH`) decodes the episode's access units and writes back only
the sampled pictures, which are tiled twenty to a JPEG contact sheet, five
across and four down, each tile 224 px wide at the recording's aspect ratio
with its time from the first picture burned in below it. One chat completion
at low reasoning effort reads every sheet, with a strict JSON schema for the
answer, and names where each subtask ends, in those seconds. The answer is
refused whole when it is empty, a description is blank, a boundary is not
after the one before, or a subtask other than the last reaches the end of
the recording; the last ends with the timeline wherever the model put it.
An episode whose sheets pass 20 MB is not asked. A judgment
(`subtask_judgment.rs`) reads the descriptions the Break down column stores
for the episode and asks jev through OpenRouter's Decisions API, one
question per subtask against one shared state (a Noul against the query for
Find similar, a Choice among the categories for Group similar), as many to
a request as its token budget carries: usually one request per episode. Its
answer is a list aligned with the subtasks, NULL where jev did not answer or
a subtask has no description; an episode Break down stored no subtasks for
is its failed answer. Each column is named by what it measures and a hash of
its call: the model, what it asks (camera, sample interval and task, or the
query or categories), and for a judgment the Break down column it judges.

The models read each episode whole, so what they are asked never depends on
anything above the node. Studio's packets carry presentation times counted
from the episode's first message, so a stored time's log time is that
message's time plus its own. Every model is named in its call, so every
answer names the model that gave it, and paid for with the deployment's
OpenRouter key (`OPENROUTER_API_KEY`), which the backend sends with its own
requests; a run that asks a model through OpenRouter is refused without it.
Literals in a view go through `SqlLiteral`.

### Rules the compiler keeps

A CTE is a name, not a stored result: Spiral pastes a copy of a step into
every place that reads it, so a step read twice runs twice. Having SpiralDB
compute a CTE read in several places once is not pursued for now; the
compiler keeps these rules instead.

- **A published view asks no model.** It reads stored answers only. A test
  compiles every path the grammar allows up to five nodes deep and checks
  that each stored column is read on the episode row once, and that each fill
  finds its episodes over the columns filled before it.
- **The reach chain reads each step once.** Re-nesting `kept` needs
  `GROUP BY`, which cannot carry other columns (Spiral has no `ANY_VALUE`),
  so the chain carries only `episode_id` and `kept`. A column is folded from
  the episode row, which a Select reading it reads again: a constant per
  node, never growing with the depth of the path.
- **Change grain on the side.** Entries unnest an answer to one row per
  thing, and a column or a Select folds them back per episode; the chain and
  the final `SELECT` stay one row per episode. Spiral has no correlated
  `LATERAL` subqueries and no list filter or map functions (`spql.list` has
  `contains`, `get`, `is_empty`, `len`, `sum`, and the aggregate `collect`),
  so work over one row's list is written as unnest, then compute over the
  rows.
- **A model is asked only about the episodes that reach its node.** A fill
  finds them over the reach chain above it.

## Preview

Every model node previews the same way. Pressing Run reads the episodes that
reach the node as the curation's own run, and the node card and the top bar
say how far it has got ("Breaking down 3 of 5 episodes"). The backend first
fills the node's stored answers for those episodes (a fill run over listed
episodes, `POST /api/curation-fills`, polled every second; Stop stops it and
keeps what it answered), then one statement reads every one of those
episodes, bound as `$1` through `$N`: the node's entries over their rows,
as the published view writes them (`CurationPath::preview`), each entry with
its episode. A preview carries what the Select's views need and nothing
bigger: Score progress its frames' times and progress rates, Track objects
each picture's time and how many objects it holds, and the subtask nodes
their subtasks with their judgments. Timelines answer as one Arrow stream
(`POST /api/queries/episodes/arrow`), subtasks as rows
(`POST /api/queries/episodes`), each episode bounded as one episode's read
is; the browser splits the answer by episode. The boxes Track objects stored
are read for the one episode opened (`track_objects/tracks.rs`).

The read asks no model, since the fill already stored every answer, so it
answers like any other query: over the 100-episode sample it is one
round trip of about the cost of one episode's. Stop pressed while it is read
drops what it answers, and its episodes wait for the next Run. A failed
episode is listed with its reason and never stops the rest; an episode whose
stored answer is empty failed when it was filled. What the preview reads is
held in the browser for the session, under the node's answer column, and is
gone after a reload. A curation opened reads back what its columns already
hold, from the top of the graph down: each node reads its episodes the same
way while it asks which of them its column holds
(`POST /api/curation-answers/filled`), so an empty answer reads as failed
and a missing one as not yet run.

A preview depends only on the node's own call, so changing a node's
settings leaves it having read nothing, and nothing below it goes out of
date: what reaches a node changes only which episodes it reads. Find similar
runs by itself whenever its query is set and the subtasks that reach it have
not been judged, as after Find similar on a row; Group similar regroups by
itself the same way once it has categories. Break down and the timelines
wait for Run, since each episode costs a model call over its video.

A Select asks nothing. The browser keeps by its conditions over the
preview's reads and the sample's facts (`select/reading.rs`), so sliding a
threshold answers at once and asks nothing again. Over the sample a percent
rank ranks the sample's episodes, so the preview's lowest 25% is of the first
100 episodes and the published view's of all of them. Tests hold the
browser's reading to the compiled SQL for every kind of condition.

Group similar's categories are configuration. One model call in the browser,
through the Studio provider, proposes four to eight from the descriptions of
the subtasks that reach the node, and the operator may rename or merge them.
Either way the node's call names them, and changing them runs the
classification again, never the proposal. A proposal numbers its categories
afresh and points every Select choosing among them at all of them; a renamed
category keeps its id, and a merged one's Selects choose the category it
joined.

## Publishing

Publish, at the end of the document bar, lists every node but the Dataset
with the name of its view. Choose the nodes to publish and give each a view
name: one plain identifier. A view is the node's table: one row per episode,
`episode_id`, `kept`, the facts, and every column of the model nodes on its
path. Publish saves the curation first, because Data judges each view
against the saved recipe. Studio then starts one fill run
(`POST /api/curation-fills`) with every column the chosen views read, each
once, and the views, records every `node → view name, statement digest` in
one edit (the SHA-256 of the statement the view was created with), and saves
once.

Before the run starts, the backend checks each view name and admits every
statement as one read-only query. The run is the backend's and outlives the
page. For each column in order it reads the episodes that reach the node,
skips those the column holds, asks about the rest (eight episodes at a time,
Break down's and the judgments' included; the progress service twenty across
runs, its containers each taking in eight uploads while the GPU scores; the
object tracking service eight, two H100 containers' worth; and retries 408s,
429s, 5xx, and timeouts with a pause, an OpenRouter one as its `Retry-After`
says),
and holds the answers in memory until they reach 4 MiB, since Spiral refuses
to change more than 16 MiB of a hosted table in one statement. Each full set,
and the rest at the end, is written while the asking goes on: appended to
the column's table by bulk ingest, without the episodes the table already
holds, so two runs filling one column at once (two previews of one call)
still leave one row per episode; writes wait for each other and for every
read of the project. A rejected video (422) is that episode's
failed answer, and so is an episode a model through OpenRouter could not
answer; a refused token, a service on another revision or a model it can't
run (409), a model it doesn't know (404), a refused OpenRouter key or an
account out of credit (401, 402), or an error ends the run once what it
answered is written. Once every column is filled it checks that
each view's statement plans (`LIMIT 0`) and stores it in the Studio SQLite
table `curated_views`, replacing the statement its name had, so a reader sees
the old view or the new one. Spiral keeps no hosted views, so Studio does:
Data reads a view by running its statement, and no other client reads it by
name. A project bundle carries none, since it carries no answers. A
restarted backend loses the answers held since the last write; publishing
again resumes past every episode already written. While a run fills a view,
Data lists it as "In progress", says which node and how many of its episodes
are answered, and opens it once the run is done. Unpublish forgets the view;
its stored answers stay.

Data opens a view through the curation that published it, which says what
each column is: it lists each episode's kept ranges, each named by the
subtask it lies in and measured by that subtask's similarity and category.
Training unnests `kept` into the intervals it trains on
([Training](training.md)).

A view is out of date once its node compiles to another statement: an edit
on its path, from the dataset down to the node. Edits to other branches, and
steps added after the node, leave it current. The canvas and Publish judge the
open curation, so an edit shows as soon as it is made; Data judges the saved
one. An out-of-date view stays as it was until the node is published again.
Removing a published node's branch waits until it is unpublished.

## Documents

Curations live in the Studio SQLite table `curations` and are saved only on an
explicit Save, with an optimistic revision check: the submitted revision must
equal the stored one, and the saved copy carries the next. A rename saves at
once, from the stored copy, so only the name moves and unsaved graph edits
stay unsaved. Any saved curation can be deleted from its row in the switcher, open or not:
Studio reads its stored copy, drops every view that copy publishes, and then
deletes it at the revision just read. A view that fails to drop stops
the delete. Deleting the open one opens a blank graph. The payload is
opaque to the backend and versioned by the frontend. Encoding 7 keeps only the
recipe: the dataset (`{id, format}`), nodes, edges, each node's configuration,
and the published nodes, each with its view's name and statement digest.
Break down keeps its camera, sample interval, and where it reads each
episode's task: the topic (`task_topic`), or the task the dataset lists
(`listed_task`); Find similar its query; Group similar its categories,
each with its id; Track objects its camera, what it tracks, and its rate; a
Select its conditions, naming the nodes they read by id. Nothing any node
read is saved. A document in an earlier encoding is refused. Each encoding
replaced every earlier one: the Studio migration that brought it in
(`m20261008_000001_curations_encoding_5`, then
`m20261011_000001_curations_encoding_6`) dropped each older curation and the
views its payload lists as published; encoding 7
(`m20261014_000001_studio_datasets`) dropped every curation and every
published view, since each read Spiral's retired Datasets.

## Why

| Decision | Why | Rejected |
| --- | --- | --- |
| No SQL node, joins, or free-form query shapes | Curation always ends in training input; analytics is not the product | A SQL escape-hatch node |
| Every node compiles to Spiral SQL, and a published view is the whole curation | One executable form: every reader sees exactly what was curated | Studio materializing results |
| Per-row model work is the backend's: a call it makes through OpenRouter, or a Modal service for video models; an authoring suggestion, such as proposed categories, stays in the browser and becomes part of the call | A view reads only what Spiral stores, and Studio owns the calls its models answer | Browser-orchestrated calls per episode; Spiral's `spql.ai` functions in fill statements |
| Model answers are stored per dataset, one column per exact call, filled by a backend run | A view over the whole dataset must not ask its models on every read, and identical calls share their work | Re-asking on every read over the first 20 episodes; waiting for Spiral to cache |
| One table per answer column, written only by appending (decided 2026-10-14) | Appends need no SQL writes: plain bulk ingest now, Spiral's transactions later | One answers table per dataset, a column added and merged in per call |
| One row per episode, with `kept` as a list of ranges (decided 2026-10-08) | Upstream SpiralDB is moving to one episode as one row; an episode's answers stay whole beside what is kept of it | One row per kept interval; one row per moment |
| Model nodes add columns; the Select is the one filter (decided 2026-10-08) | One place owns every cutoff, and any column can be cut on, such as duration per bottle | Each measure keeping by its own rule |
| Column meaning is the compiler's, by node id | A Select, Data, and Training read a column by what wrote it, and a renamed category is still the same column | Inspecting the view's types at run time |
| The reach chain carries only `kept`, and columns fold from the episode row | Folding `kept` needs `GROUP BY`, which carries no other column, and carrying big answers through an unnest copies them per row | Window aggregates carrying every column |
| A preview reads the node's own entries for the episodes it lists, in one statement | Its answers are the ones a publish reuses, and it no longer depends on what reached the node | Previewing over the parent's rows as `VALUES` |
| Preview reads the first 100 episodes in recording order | Fast enough to iterate on | A random sample |
| Curation only chooses data | Sample rate, cameras, image size, and normalization are the policy's input contract, which rollout replays | Shape nodes in Curation |
| Publish is explicit and named; Save stays local | Exploratory leaves must not become views readers depend on, and readers need a stable name | Auto-publishing every leaf |
| The frontend compiles; the backend admits read-only statements, fills the columns they name, and stores the view | The backend stays thin and the browser never sends DDL | The backend compiles the graph |

## Not yet

- Answer columns no curation reads any more stay; collecting them is
  later. Cancelling a running query is Spiral's to add.
- Training reads published views ([Training](training.md)): it resamples
  each kept range onto its own clock, and the checkpoint carries how the
  model saw the data. Exposure normalization and colour jitter are not
  there yet.
- A run pins what it read: its uploads are named by the dataset version they
  were read from, though its record keeps only the view's name.
