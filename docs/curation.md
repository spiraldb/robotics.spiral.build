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
are named by what measured them (`pace`, and later `subtask`,
`subtask_category`, `similarity`). An episode-level output shows the facts its
Selects read, such as `duration_seconds`.

Intervals of one episode never overlap: Measures only cut intervals, and
Selects only drop them. The view holds no stream data; cameras and joint
states stay in the recording.

## Compiling a path

`CurationPath` in `activities/curation/path.rs` walks from the dataset down to
one node and chains each node's step as `WITH` entries. Each node kind writes
its own step beside its configuration:

| Node | Step | Where |
| --- | --- | --- |
| Dataset | Every episode of `spql.mcap.episodes` as one whole interval, with its facts, through the episode listing `Episode` uses everywhere | `graph/dataset.rs` |
| Select | A `WHERE` from the same `Selection` the browser filters with. A pace rule keeps moments with no pace too, then joins touching moments into stretches carrying their mean pace | `document/selection.rs` |
| Identify unproductive moments | `spql.ai.score_progress` over the top camera's packet video, one moment per scored frame from its time to the next frame's, cut to the intervals that reach the node | `moments/scoring.rs` |
| Break down, Find similar, Group similar | Not compiled yet. They wait for `spql.ai.breakdown_task`, `spql.ai.similarity`, and `spql.ai.classify` | |

The scorer reads each episode whole, so what it is asked never depends on
anything above the node. Literals go through `SqlLiteral`.

The same statement runs two ways. The **preview** binds one episode as `$1`
through `/api/queries/episode/arrow`. The **published** view reads every
episode. Only the dataset's source differs: the preview admits the bound
episode where the dataset is listed and where a camera topic is read, so it
never reads the rest of the dataset.

Spiral's SQL is not plain DataFusion. Only aggregate and ranking functions
take `OVER`, so the next frame's time is `MIN(…) OVER (… ROWS BETWEEN 1
FOLLOWING AND 1 FOLLOWING)` and stretches are found by the difference of two
`ROW_NUMBER`s. Scores are unnested straight from `score_progress`, before any
join, because the engine does not join lists of structs.

## Preview

Selecting Identify unproductive moments and pressing Run scores the episodes
that reach it, one query per episode, as the curation's own run. The node card
and the top bar say how far it has got ("Scoring 12 of 100 episodes"). Stop
takes effect between episodes, since Spiral cannot cancel a running query yet.
A failed episode is listed with its reason and never stops the rest. What the
preview reads is held in the browser for the session and is gone after a
reload.

A Select's slider filters the preview rows of the node above it in the
browser, so it answers at once. The same `Selection` renders the `WHERE` the
published view keeps by, and a test holds the two to the same rows.

## Publishing

On an output's workspace, Publish asks for a view name: one plain identifier,
at the catalog root. With unsaved edits it asks you to save first, because the
view is compiled from the saved recipe. Studio then sends the compiled
statement to `POST /api/curations/publish`, records `node → view name,
revision` in the curation, and saves.

The backend checks the name, admits the statement as one read-only query,
refuses a name that a table or a Dataset holds, and runs `DROP VIEW IF EXISTS`
then `CREATE VIEW` in one transaction. Unpublish drops the view.

A later save that changes the recipe moves the curation past the published
revision, and the output's card then reads "Out of date" until it is
published again. A published output stays an output: adding a step after it,
or removing its branch, waits until it is unpublished.

## Documents

Curations live in the Studio SQLite table `curations` and are saved only on an
explicit Save, with an optimistic revision check: the submitted revision must
equal the stored one, and the saved copy carries the next. The payload is
opaque to the backend and versioned by the frontend. Encoding 2 keeps only the
recipe for compiled nodes, plus the published outputs. Break down subtasks,
Find similar, and Group similar still store what they measured until they
compile. Curations saved in encoding 1 were deleted, with the saved
workflows, by the migration that retired Workflow.

## Not yet

- Break down subtasks, Find similar subtasks, and Group similar subtasks run
  as before, in the browser, and a path through them cannot be published
  until Spiral has their functions.
- Every read of a published view repeats its model calls; caching them is
  Spiral's to add.
- Training does not read published views yet.
