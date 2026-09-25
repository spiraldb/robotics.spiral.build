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
intervals with properties. Every node is one of two kinds:

- **Measures** add a property to each interval, or cut intervals into finer
  ones: Break down subtasks, Find similar subtasks, Group similar subtasks,
  and Identify unproductive moments.
- **Selects** keep the intervals whose property passes their rule. What a
  Select keeps by comes from the nearest node above it that is not a Select:
  the dataset's episode facts, a search's similarity, a grouping's category,
  or the moments' pace.

Every Select that nothing follows is an **output**. Its relation is what a
published view holds.

## What an edge carries

An edge is a relation with the columns `episode_id`, `start_ns`, `end_ns`,
and then one column per property. Times are MCAP log time in nanoseconds, so a
reader can join the recording's streams without converting them. Properties
are named by what measured them: `pace`, `subtask`, `similarity`, and
`subtask_category`, which holds the category and its probability. A measure
repeated on one path writes `similarity_2`, `similarity_3`, and so on, and a
Select reads the column of the nearest measure of its kind. An episode-level
output shows the facts its Selects read, such as `duration_seconds`; Break
down and Identify unproductive moments start their own properties, and the
text judgments add theirs to what reaches them.

Intervals of one episode may overlap: Break down can return subtasks that
happen at the same time, such as each arm doing something different, and
keeps them in order of start. A reader that samples moments, such as
Training's resampler, decides what an instant inside two intervals means.
The view holds no stream data; cameras and joint states stay in the
recording.

## Compiling a path

`CurationPath` in `activities/curation/path.rs` walks from the dataset down to
one node and chains each node's step as `WITH` entries. Each node kind writes
its own step beside its configuration:

| Node | Step | Where |
| --- | --- | --- |
| Dataset | Every episode of `spql.mcap.episodes` as one whole interval, with its facts, through the episode listing `Episode` uses everywhere | `graph/dataset.rs` |
| Select | A `WHERE` from the same `Selection` the browser filters with. A pace rule keeps moments with no pace too, then joins touching moments into stretches carrying their mean pace | `document/selection.rs` |
| Identify unproductive moments | `spql.ai.score_progress` over the top camera's packet video, one moment per scored frame from its time to the next frame's, cut to the intervals that reach the node | `moments/scoring.rs` |
| Break down subtasks | `spql.ai.breakdown_task` over the chosen camera's packet video, with the configured model and sample interval, and the episode's first `/instruction` message as its `task` when the dataset records one; its subtasks unnested, moved from the video's timeline to log time, and cut to the intervals that reach the node | `annotate/breakdown.rs` |
| Find similar subtasks | `spql.ai.similarity(subtask, query)` as `similarity` | `similar/judgment.rs` |
| Group similar subtasks | `spql.ai.classify(subtask, categories)` as `subtask_category`, the categories written into the statement as a literal list | `similar/judgment.rs` |

The scorer and Break down read each episode whole, so what they are asked
never depends on anything above the node. Studio's packets carry presentation
times counted from the episode's first message, so a subtask's log time is
that message's time plus its own. `breakdown_task` answers with a list of
structs, which is unnested straight from the call; the instruction is joined
to the videos before it, since Spiral joins scalar columns only. The text
judgments are asked of `typesafe/jev-1.13`, named in the statement so every
answer names the model that gave it. Every model function is paid for with
Studio's `LLM_API_KEY`, which the backend passes to the driver as
`spiral.openrouter_api_key` (see [the database boundary](database-boundary.md)).
Literals go through `SqlLiteral`; a
function parameter that must be a constant, such as `sample_interval`, takes a
bare number, because Spiral types a `CAST` as nullable.

Each step is written once, over whatever relation reaches it, and is read two
ways. The **published** view chains every step from the dataset down into one
statement over every episode. A **preview** writes only the node's own step,
over the rows its parent's preview already holds, supplied as `VALUES` in
place of the parent's entry (`CurationPath::preview`), so nothing above the
node runs again. Two more things differ. A previewed step that reads a
recording admits only the episode bound as `$1` wherever a topic is read. And
the preview passes `on_error => 'fail'` to every `spql.ai` call that takes it,
so an episode the model cannot answer fails with its reason; the view keeps
the functions' default, a NULL for that row, so one episode never ends a read
of thousands.

The next frame's time is `MIN(…) OVER (… ROWS BETWEEN 1 FOLLOWING AND 1
FOLLOWING)`, which returns exactly what `LEAD` does (SpiralDB has `LEAD` and
`LAG` since spiraldb#7387), and stretches are found by the difference of two
`ROW_NUMBER`s. Scores are unnested straight from `score_progress`.

### Rules the compiler keeps

A CTE is a name, not a stored result: Spiral pastes a copy of a step into
every place that uses it, so a step used twice runs twice. For a cheap step,
such as the episode listing, that is harmless and keeps each use's filters
pushed down to the scan. For a step with a model call it doubles the model's
work: on Dataset → Identify unproductive moments → Select → Break down, the
moments Select is read once to decide which episodes Break down sees and once
to clip its subtasks, and the plan runs `spql.ai.score_progress` twice per
episode (three times with an `/instruction` topic).

- **A step containing a model call is used once.** When a path needs its
  result in two places, keep the result as a column on the episode rows
  instead of re-reading the step, or rely on the engine computing a
  multiply-used expensive CTE once (the CTE-materialization change in
  SpiralDB, pending).
- **Per-episode work stays on the episode rows.** A model call over one
  episode's video is a column in the projection over `spql.mcap.episodes`;
  the episode's start time and instruction ride along as columns rather than
  being joined back in after the model call.
- **Change grain once, as late as possible.** Unnesting to subtask or moment
  rows comes after the per-episode columns, and Find similar and Group similar
  add their columns on those rows. Spiral has no correlated `LATERAL`
  subqueries and no list filter or map functions, so work over one row's list
  is written as unnest, compute, then `GROUP BY` the episode.

## Preview

Every measurement previews the same way. Pressing Run reads the episodes that
reach the node as the curation's own run, and the node card and the top bar
say how far it has got ("Breaking down 3 of 5 episodes"). What reaches the
node is its parent's preview rows, written into the statement (`preview/`):

- **Break down and Identify unproductive moments** read one recording per
  query, eight queries in flight. Each is handed the intervals of its episode
  that reach the node, with the episode bound as `$1`, through
  `/api/queries/episode` (subtasks) or `/api/queries/episode/arrow` (moments).
  After a Select of moments these are the stretches it keeps, as its step
  joins them.
- **Find similar and Group similar** read no recording, so they judge the
  subtasks of many episodes in one `/api/queries` statement: as many as fit
  in 60 KiB and 1,000 rows, which is one statement for a sample of short
  episodes. One call over many rows costs the same model time as many small
  ones and saves the round trips. The ids travel as text, because Spiral
  casts no text to a UUID and only `$1` binds one, and `subtask_category`
  travels as its two fields, rebuilt with `STRUCT_PACK`, because `VALUES`
  takes literals only.

Stop starts no further query and lets the ones in flight finish, since Spiral
cannot cancel a running query. A failed episode is listed with its reason and
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

A Select's slider filters the preview rows of the node above it in the
browser, so it answers at once. The same `Selection` renders the `WHERE` the
published view keeps by, and tests hold the two to the same rows for every
kind of rule. Tests also hold each measure's preview to its published step
over the parent's rows; run live with literal stand-ins for the model's
answers, the two readings return the same rows for every measure.

## Publishing

On an output's workspace, Publish asks for a view name: one plain identifier,
at the catalog root. With unsaved edits it asks you to save first, because the
view is compiled from the saved recipe. Studio then sends the compiled
statement to `POST /api/curations/publish`, records `node → view name,
revision` in the curation, and saves.

The backend checks the name, admits the statement as one read-only query,
refuses a name that a table or a Dataset holds, and runs one `CREATE OR
REPLACE VIEW`, so a reader sees the old view or the new one. (Spiral refuses a
`DROP VIEW` and a `CREATE VIEW` of one name in one transaction as a namespace
conflict once the view exists.) Unpublish drops the view.

A later save that changes the recipe moves the curation past the published
revision, and the output's card then reads "Out of date" until it is
published again. A published output stays an output: adding a step after it,
or removing its branch, waits until it is unpublished.

## Documents

Curations live in the Studio SQLite table `curations` and are saved only on an
explicit Save, with an optimistic revision check: the submitted revision must
equal the stored one, and the saved copy carries the next. The payload is
opaque to the backend and versioned by the frontend. Encoding 2 keeps only the
recipe: nodes, edges, each node's configuration, and the published outputs.
Break down keeps its camera, model, sample interval, and the topic it reads
each episode's task from; Find similar its query; Group similar its
categories. Nothing any node read is saved. A scratch document that still
carries subtasks, groupings, rankings, or an annotation's episode lists is
refused. Curations saved in encoding 1 were deleted, with the saved
workflows, by the migration that retired Workflow.

## Not yet

- Every read of a published view repeats its model calls; caching them is
  Spiral's to add.
- Training does not read published views yet.
