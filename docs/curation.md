# Curation

Curation chooses the data to train on: which stretches of which episodes, and
what to call them. A curation is a graph rooted at one dataset. You build it
against a preview of the dataset's first 100 episodes, and you publish what it
selects as a named view in the Spiral catalog. The view is a read-only query
Spiral runs over the whole dataset whenever it is read, with one row per kept
interval of an episode.

How a model sees that data (sample rate, cameras, image size, normalization)
is Training's concern, not Curation's. See
[the merge plan](curation-merge-plan.md) for the decisions behind this model.

## The graph

Every node except the dataset has one parent, and each edge carries episode
intervals with properties. A node is what it asks, and its **rule**: the knob
it keeps part of the answer by, tuned in its workspace.

- **Measures** add a property to each interval, or cut intervals into finer
  ones: Break down subtasks, Find similar subtasks, Group similar subtasks,
  and Score progress. Score progress keeps the moments progressing at least
  its threshold, joined into stretches, and Find similar the subtasks at
  least its cutoff likely to be what it asks.
- **Selects** measure nothing: their rule is all they are. A Select after the
  dataset keeps the episodes that meet every one of its conditions on their
  recorded facts; a Select after Group similar keeps the chosen categories.
  One Select holds all its conditions, so none follows another.

What may follow a node:

| Node | Followed by |
| --- | --- |
| Dataset | Break down, Score progress, Select |
| Select of episodes | Break down, Score progress |
| Score progress | Break down, which reads the stretches it keeps |
| Break down | Find similar, Group similar |
| Find similar | Find similar, Group similar, over the subtasks it keeps |
| Group similar | Select of categories |
| Select of categories | Find similar, Group similar |

Every Score progress, Find similar, Group similar, and Select node is an
**output**, whatever follows it: its relation is what a published view holds.
Break down is not one: its descriptions are free text that differs from
episode to episode until a search or a grouping reads them.

## What an edge carries

An edge is a relation with the columns `episode_id`, `start_ns`, `end_ns`,
and then one column per property. Times are MCAP log time in nanoseconds, so a
reader can join the recording's streams without converting them. Properties
are named by what measured them: `progress_rate`, `subtask`, `similarity`, and
`subtask_category`, which holds the category and its probability. A measure
repeated on one path writes `similarity_2`, `similarity_3`, and so on, and a
Select reads the column of the nearest measure of its kind. An episode-level
output shows the facts its Selects read, such as `duration_seconds`; Break
down and Score progress start their own properties, and the
text judgments add theirs to what reaches them.

Intervals of one episode may overlap: Break down can return subtasks that
happen at the same time, such as each arm doing something different, and
keeps them in order of start. A reader that samples moments, such as
Training's resampler, decides what an instant inside two intervals means.
The view holds no stream data; cameras and joint states stay in the
recording.

## Compiling a path

`CurationPath` in `activities/curation/path.rs` walks from the dataset down to
one node. Each node kind writes its own part beside its configuration, in two
halves (`PathFragment`): what it asks a model about each whole episode, as a
column of that episode's row (`EpisodeRow`), and its step over the interval
rows that reach it.

| Node | On the episode row | Step over the rows that reach it | Where |
| --- | --- | --- | --- |
| Dataset | | Every episode of `spql.mcap.episodes` as one whole interval, with its facts, through the episode listing `Episode` uses everywhere | `graph/dataset.rs` |
| Select | | Its rule alone: a `WHERE` of every condition, or of the categories, from the same `Selection` the browser filters with | `document/selection.rs` |
| Score progress | `scores`: `spql.ai.score_progress` over the top camera's packet video | The scores unnested, one moment per scored frame from its time to the next frame's, cut to the interval of the row they arrived on; then its rule keeps the moments at least its threshold, and those with no progress rate, and joins touching ones into stretches carrying their mean progress rate | `score_progress/scoring.rs` |
| Break down subtasks | `subtasks`: `spql.ai.breakdown_task` over the chosen camera's packet video, with the configured model and sample interval, and the episode's first `/instruction` message as its `task` when the dataset records one | The subtasks unnested, moved from the video's timeline to log time, and cut to the interval of the row they arrived on | `annotate/breakdown.rs` |
| Find similar subtasks | | `spql.ai.similarity(subtask, query)` as `similarity`; then its rule keeps the subtasks at least its cutoff | `similar/judgment.rs` |
| Group similar subtasks | | `spql.ai.classify(subtask, categories)` as `subtask_category`, the categories written into the statement as a literal list | `similar/judgment.rs` |

A node's rule is its own entry after its step, named after it
(`score_progress_1_kept`); a Select's rule is its whole step (`select_1`).

A published view is one stream of episode rows (decided 2026-09-25). The
dataset's listing and its Select of episodes come first; a Select keeps by
the nearest measure above it, so this is the only Select that can come
before the first measure, and it is the `WHERE` on the episode rows.
Where the path leaves whole episodes, it builds each episode's row once:

```text
dataset_0, select_1       the listing with its facts, and the Selects of episodes
camera_video_1            each camera a model on the path watches, read once
instruction_1             each episode's first instruction, when Break down reads one
episodes                  one row per episode: origin_ns, scores, subtasks; no video
score_progress_…, …_kept each step, and each rule after it, reads the entry above it once
break_down_…, find_…      unnesting the answer it asked for when it reaches it
```

The row carries the episode's first message as `origin_ns` and one column per
model answer. The answers ride on the interval rows until the step of the
node that asked for them unnests them: on Dataset → Score progress → Break
down, `subtasks` rides on every moment and every stretch. Spiral joins and windows rows that carry these lists of structs as
readily as scalar ones.

The scorer and Break down read each episode whole, so what they are asked
never depends on anything above the node. Studio's packets carry presentation
times counted from the episode's first message, so a subtask's log time is
that message's time plus its own. The text judgments are asked of
`typesafe/jev-1.13`, named in the statement so every answer names the model
that gave it. Every model function is paid for with Studio's `LLM_API_KEY`,
which the backend passes to the driver as `spiral.openrouter_api_key` (see
[the database boundary](database-boundary.md)). Literals go through
`SqlLiteral`; a function parameter that must be a constant, such as
`sample_interval`, takes a bare number, because Spiral types a `CAST` as
nullable.

Each step is written once and read two ways. The **published** view chains
every step from the dataset down into one statement over every episode. A
**preview** writes only the node, over the rows its parent's preview already
holds, supplied as `VALUES` in place of the parent's entry
(`CurationPath::preview`), so nothing above the node runs again. Those rows
are flat: a previewed node that asks each episode anything builds its own row
for the episode bound as `$1`, reading the bound episode wherever a topic is
read, and joins it to them before the same step. And the preview passes
`on_error => 'fail'` to every `spql.ai` call that takes it, so an episode the
model cannot answer fails with its reason; the view keeps the functions'
default, a NULL for that row, so one episode never ends a read of thousands.
The preview leaves the node's rule out: the browser keeps by it over what the
step read, so sliding a threshold asks nothing again.

The next frame's time is `LEAD`, and the last frame lasts as long as the one
before it (`LAG`); the frames and the window over them are separate entries.
Stretches are found by the difference of two `ROW_NUMBER`s, and each is
measured with `MIN`, `MAX` and `AVG` over a window of the stretch, its first
moment standing for it, so the answers riding on the moments ride on with
their stretches. `GROUP BY` cannot carry them: Spiral has no `ANY_VALUE`, and
refuses `MIN` of a list and `FIRST` in a grouped aggregate.

### Rules the compiler keeps

A CTE is a name, not a stored result: Spiral pastes a copy of a step into
every place that reads it, so a step read twice runs twice. For a cheap step,
such as the episode listing, that is harmless and keeps each read's filters
pushed down to the scan; the episode row reads the Selects of episodes once
for its rows and once for each topic it reads. For a step with a model call
it repeats the model's work. Until 2026-09-25 each model step read the
relation that reached it twice, to choose the episodes it read and to clip
its answers, and Dataset → Score progress → Select → Break
down ran `spql.ai.score_progress` twice per episode and read the camera
three times. Having SpiralDB compute a CTE read in several places once is not
pursued for now; the compiler keeps these rules instead.

- **An entry containing a model call is read once.** Every step reads the
  entry above it once. A test compiles every path of up to six nodes the
  grammar allows and counts the reads of each such entry, and each model
  node's call.
- **Per-episode work stays on the episode row.** A model call over one
  episode's video is a column of the episode's row, built on the episode
  listing; the episode's first message and instruction are columns there
  too, rather than joined back in after the call. A camera two models watch
  is read once.
- **Change grain once, as late as possible.** Unnesting to moment or subtask
  rows comes after the episode row, and the answers a later step needs ride
  along as list columns. Find similar and Group similar add their columns on
  subtask rows. Spiral has no correlated `LATERAL` subqueries and no list
  filter or map functions (`spql.list` has `contains`, `get`, `is_empty`,
  `len`, `sum`, and the aggregate `collect`), so work over one row's list is
  written as unnest, then compute over the rows.
- **Only a Select of episodes spares an episode its model calls.** A model's
  column is on the episode row, before any interval is cut, so a rule below
  a measure keeps rows but never skips a call. On Score progress → Break
  down, the view asks `breakdown_task` of every episode, including one where
  no frame passes Score progress's threshold. Its rows are still the
  preview's, which asks only the episodes that reach Break down; only the
  cost differs.

## Preview

Every measurement previews the same way. Pressing Run reads the episodes that
reach the node as the curation's own run, and the node card and the top bar
say how far it has got ("Breaking down 3 of 5 episodes"). What reaches the
node is its parent's preview rows, written into the statement (`preview/`):

- **Break down and Score progress** read one recording per
  query. Each is handed the intervals of its episode that reach the node,
  with the episode bound as `$1`, and answers as rows (subtasks) or as one
  Arrow stream (moments). Touching intervals are handed on joined, so below
  Score progress Break down is handed the stretches its rule keeps, never a
  row per frame.
- **Find similar and Group similar** read no recording, so they judge the
  subtasks of many episodes in one statement: as many as fit
  in 60 KiB and 1,000 rows, which is one statement for a sample of short
  episodes. One call over many rows costs the same model time as many small
  ones and saves the round trips. The ids travel as text, because Spiral
  casts no text to a UUID and only `$1` binds one, and `subtask_category`
  travels as its two fields, rebuilt with `STRUCT_PACK`, because `VALUES`
  takes literals only.

A pass hands every query to the backend as one run
(`POST /api/curation-previews`), which answers at once and works through them
eight at a time. The browser polls it every second
(`GET /api/curation-previews/{id}`) and fetches each answer as it lands
(`…/answers/{query}`), so no request of the page waits on a model: a query can
take a minute, and the browser opens only six connections to the backend.

Stop (`…/stop`) ends the run at once: it starts no further query, and the
backend drops the ones in flight, whose episodes read as never run and wait
for the next Run. Spiral cannot cancel a statement it is computing, so a
dropped query finishes the batch it is on and its answer is thrown away; the
model calls it made are still spent. A run nobody polls for 30 seconds only
starts nothing more, since the page that wanted it has gone, and a settled
run is dropped once it has gone unread that long. Runs live in the backend's
memory, so a restarted backend holds none. A failed episode is listed with its reason and
never stops the rest; a failed batch fails each episode in it, since
`on_error => 'fail'` fails the statement. What the preview reads is held in
the browser for the session and is gone after a reload.

Each read keeps what went into it: the configuration of the measure above
and a print of the rows it read. Editing a node's configuration, or running
it again, changes what reaches every node below it, and those nodes read "out
of date" until they run again, from their parent's current rows. A node's own
configuration change leaves it having read nothing. Find similar runs by
itself whenever its query is set and the subtasks that reach it have not been
judged, as after Find similar on a row or on opening an out-of-date search;
Group similar regroups by itself the same way once it has categories. Break
down and the moments wait for Run, since each episode costs a model call over
its video.

Group similar's categories are configuration. One model call in the browser,
through the Studio provider, proposes four to eight from the descriptions of
the subtasks that reach the node, and the operator may rename or merge them.
Either way the node's statement names them, and changing them runs the
classification again, never the proposal.

A rule's slider filters preview rows in the browser, so it answers at once:
a Select's the rows of the node above it, a measure's its own. The same
`Selection` renders the `WHERE` the published view keeps by, and tests hold
the two to the same rows for every kind of rule. Tests also hold each measure's preview to its published step
over the parent's rows; run live with literal stand-ins for the model's
answers, the two readings return the same rows for every measure.

## Publishing

Publish, at the end of the document bar, lists every output with the name of
its view. Choose the outputs to publish and give each a view name: one plain
identifier, at the catalog root. With unsaved edits it asks you to save
first, because each view is compiled from the saved recipe. Studio then sends
each compiled statement to `POST /api/curations/publish` in turn, records
every `node → view name, revision` it created in one edit, and saves once: a
publication is recorded against the saved revision, and the save that records
it must come next.

The backend checks the name, admits the statement as one read-only query,
refuses a name that a table or a Dataset holds, and runs one `CREATE OR
REPLACE VIEW`, so a reader sees the old view or the new one. (Spiral refuses a
`DROP VIEW` and a `CREATE VIEW` of one name in one transaction as a namespace
conflict once the view exists.) Unpublish drops the view.

A later save that changes the recipe moves the curation past the published
revision, and Publish then lists the output's view as out of date until it
is published again. Removing a published output's branch waits until it is
unpublished; a step may follow it, and it stays an output.

## Documents

Curations live in the Studio SQLite table `curations` and are saved only on an
explicit Save, with an optimistic revision check: the submitted revision must
equal the stored one, and the saved copy carries the next. The payload is
opaque to the backend and versioned by the frontend. Encoding 3 keeps only the
recipe: nodes, edges, each node's configuration and rule, and the published
outputs. Break down keeps its camera, model, sample interval, and the topic
it reads each episode's task from; Find similar its query and cutoff; Group
similar its categories; Score progress its threshold; a Select its
conditions or categories. Nothing any node read is saved. A document in an
earlier encoding is refused: encoding 2 kept the thresholds in Selects of
their own, and curations saved in encoding 1 were deleted, with the saved
workflows, by the migration that retired Workflow.

## Not yet

- Every read of a published view repeats its model calls; caching them is
  Spiral's to add.
- Training does not read published views yet.
