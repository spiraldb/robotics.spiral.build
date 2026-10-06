# Curation

Curation chooses the data to train on: which stretches of which episodes, and
what to call them. A curation is a graph rooted at one dataset. You build it
against a preview of the dataset's first 100 episodes, and you publish what it
selects as a named view: a read-only query Studio keeps and Spiral runs
whenever it is read, with one row per kept interval of an episode,
over every episode of the dataset. Every model answer is stored: publishing
asks each model about the episodes that reach its node once, in the
background, and writes the answers into the dataset's answers table, which
the view reads without asking any model again.

How a model sees that data (sample rate, cameras, image size, normalization)
is Training's concern, not Curation's. [Why](#why) records the decisions
behind this model.

## The graph

Every node except the dataset has one parent, and each edge carries episode
intervals with properties. A node is what it asks, and its **rule**: the knob
it keeps part of the answer by, tuned in its workspace.

- **Measures** add a property to each interval, or cut intervals into finer
  ones: Break down subtasks, Find similar subtasks, Group similar subtasks,
  and Score progress. Score progress keeps the moments progressing at least
  its threshold, joined into stretches; Find similar the subtasks at least
  its cutoff likely to be what it asks; and Group similar the subtasks of
  the categories it keeps, every one until some are unchecked.
- **Selects** measure nothing: their rule is all they are. A Select follows
  the dataset and keeps the episodes that meet every one of its conditions
  on their recorded facts: duration, message count, channel count, and file
  size. It never reads what a model answered; each measure keeps by its own
  rule. One Select holds all its conditions, so none follows another.

What may follow a node:

| Node | Followed by |
| --- | --- |
| Dataset | Break down, Score progress, Select |
| Select of episodes | Break down, Score progress |
| Score progress | Break down, which reads the stretches it keeps |
| Break down | Find similar, Group similar |
| Find similar | Find similar, Group similar, over the subtasks it keeps |
| Group similar | Find similar, Group similar, over the subtasks it keeps |

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
repeated on one path writes `similarity_2`, `similarity_3`, and so on, and
its rule reads its own column. An episode-level
output shows the facts its Selects read, such as `duration_seconds`; Break
down and Score progress start their own properties, and the
text judgments add theirs to what reaches them.

Break down's subtasks cover each episode end to end: each starts where the
previous one ends, with no gaps and no overlaps, so reaching, pausing,
handing over, and retracting are subtasks too, and two arms acting at once
share one subtask. Cut to the intervals that reached the node, they still
never overlap, so a reader that samples moments, such as Training's
resampler, finds at most one subtask at any instant.
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
| Select | | Its rule alone: a `WHERE` of every condition, from the same `Selection` the browser filters with | `document/selection.rs` |
| Score progress | Its stored scores: the Modal service's WARP scores of the top camera's packets | The scores unnested, one moment per scored frame from its time to the next frame's, cut to the interval of the row they arrived on; then its rule keeps the moments at least its threshold, and those with no progress rate, and joins touching ones into stretches carrying their mean progress rate | `score_progress/scoring.rs` |
| Break down subtasks | Its stored subtasks: `spql.ai.breakdown_task` over the chosen camera's packet video, with Studio's annotation model and the configured sample interval, and the episode's first `/instruction` message as its `task` when the dataset records one | The subtasks listed with their place in the answer (`subtask_index`), moved from the video's timeline to log time, and cut to the interval of the row they arrived on | `annotate/breakdown.rs` |
| Find similar subtasks | Its stored similarities: `spql.ai.similarity(subtask, query)` of every subtask of the Break down above, one list per episode aligned with its subtasks | Each subtask's similarity at its `subtask_index` as `similarity`; then its rule keeps the subtasks at least its cutoff | `similar/judgment.rs` |
| Group similar subtasks | Its stored categories: `spql.ai.classify(subtask, categories)` the same way, the categories written into the call as a literal list | Each subtask's category at its `subtask_index` as `subtask_category`; then its rule keeps the subtasks of the categories it keeps | `similar/judgment.rs` |

A node's rule is its own entry after its step, named after it
(`score_progress_1_kept`); a Select's rule is its whole step (`select_1`).

A published view is one stream of episode rows (decided 2026-09-25). The
dataset's listing and its Select of episodes come first; a Select only
follows the dataset, so it always comes before the first measure, and it is
the `WHERE` on the episode rows. Where the path leaves whole episodes, it
builds each episode's row once, joined to the dataset's answers table:

```text
dataset_0, select_1       the listing with its facts, and the Selects of episodes
episodes                  one row per episode: origin_ns and each stored answer the path reads
score_progress_…, …_kept each step, and each rule after it, reads the entry above it once
break_down_…, find_…      unnesting or indexing the answer it reads when it reaches it
```

The row carries the episode's first message as `origin_ns` and one column per
stored answer. The answers ride on the interval rows until the step of the
node that reads them: on Dataset → Score progress → Break down, the subtasks
ride on every moment and every stretch. A judgment's list rides on to the
end, so a node below asking the same reads it too. Spiral joins and windows
rows that carry these lists of structs as readily as scalar ones.

### Stored answers

Each dataset has an answers table, `dataset_answers_<id>` beside
`dataset_<id>`: one row per episode keyed by `episode_id`, created at import
from the Dataset's first refresh and given a row by each append. Every model
call a node makes is one column, named for what it measures and six hex
digits of a SHA-256 of the exact call (`score_progress_3f9a2c`): the function
or service, the model revision, the Modal service revision, the arguments,
the camera or instruction
topic, and for a judgment the Break down column it reads. Identical calls
across curations share a column. A value is `STRUCT(value …)`: a NULL struct
is an episode not asked yet, a NULL `value` one the model could not answer,
which is never asked again. Unpublishing drops only the view.

`CurationPath::fills` compiles, for each node that asks a model in path
order, its column, the episodes that reach it (the path above compiled over
the columns already filled, `SELECT DISTINCT episode_id`), and how one
episode's answer is computed: a statement over the episode bound as `$1`,
or, for Score progress, the Modal service. Break down's statement asks its
model once per episode, and a judgment's judges the episode's whole stored
list of subtasks in one call (the list forms of `spql.ai.similarity` and
`spql.ai.classify`), which answers with a list aligned with them. Break down
lists its subtasks with `spql.range` and `spql.list.get`, each carrying its
place (`subtask_index`), and a judgment reads each subtask's answer at that
place; Spiral has no `WITH ORDINALITY`.

The scorer and Break down read each episode whole, so what they are asked
never depends on anything above the node. Studio's packets carry presentation
times counted from the episode's first message, so a subtask's log time is
that message's time plus its own. The text judgments are asked of
`typesafe/jev-1.13`, named in the statement so every answer names the model
that gave it. Every model function is paid for with the deployment's OpenRouter key,
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
for the episode bound as `$1`, reading its stored answers, and joins it to
them before the same step. No preview asks a model: it fills the node's
column for the episodes it reads first. A fill keeps the functions' NULL for
an answer they cannot give, which it stores as the episode's failed answer,
so one episode never ends a fill of thousands. The preview leaves the node's
rule out: the browser keeps by it over what the step read, so sliding a
threshold asks nothing again.

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

- **A published view asks no model.** Every step reads the entry above it
  once, and the answers table is joined to the episode row once. A test
  compiles every path of up to six nodes the grammar allows and checks that
  no view calls `spql.ai`, that each stored column is read once, and that
  each fill finds its episodes over the columns filled before it.
- **Per-episode work stays on the episode row.** A stored answer, or a
  preview's model call over one episode's video, is a column of the
  episode's row, built on the episode listing; the episode's first message
  and instruction are columns there too, rather than joined back in after
  the call. A camera two models watch is read once.
- **Change grain once, as late as possible.** Unnesting to moment or subtask
  rows comes after the episode row, and the answers a later step needs ride
  along as list columns. Find similar and Group similar add their columns on
  subtask rows. Spiral has no correlated `LATERAL` subqueries and no list
  filter or map functions (`spql.list` has `contains`, `get`, `is_empty`,
  `len`, `sum`, and the aggregate `collect`), so work over one row's list is
  written as unnest, then compute over the rows.
- **A model is asked only about the episodes that reach its node.** A fill
  finds them over the columns above, so on Score progress → Break down,
  `breakdown_task` is asked of the episodes with a stretch that passes Score
  progress's threshold, and no other.

## Preview

Every measurement previews the same way. Pressing Run reads the episodes that
reach the node as the curation's own run, and the node card and the top bar
say how far it has got ("Breaking down 3 of 5 episodes"). The backend first
fills the node's stored answers for those episodes (a fill run over listed
episodes, `POST /api/curation-fills`, polled every second; Stop stops it and
keeps what it answered), then each episode is read with one query, the
episode bound as `$1`, handed the intervals of it that reach the node, its
parent's preview rows written into the statement (`preview/`). It answers as
rows (subtasks) or as one Arrow stream (moments). Touching intervals are
handed on joined, so below Score progress Break down is handed the stretches
its rule keeps, never a row per frame; `subtask_category` travels as its two
fields, rebuilt with `STRUCT_PACK`, because `VALUES` takes literals only.

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
never stops the rest. What the preview reads is held in
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
classification again, never the proposal. Which of them the node keeps is its
rule, and the document keeps the two together: a proposal keeps every
category, a renamed category keeps its standing under its new name, a merged
one leaves the rule with the category it joined, and a rule naming a
category the grouping does not have is refused.

A rule's slider or checkbox filters preview rows in the browser, so it
answers at once: a Select's the dataset's episodes, a measure's its own
rows. The same
`Selection` renders the `WHERE` the published view keeps by, and tests hold
the two to the same rows for every kind of rule. Tests also hold each measure's preview to its published step
over the parent's rows; run live with literal stand-ins for the model's
answers, the two readings return the same rows for every measure.

## Publishing

Publish, at the end of the document bar, lists every output with the name of
its view. Choose the outputs to publish and give each a view name: one plain
identifier. Publish saves the curation first, because
each view is compiled from the saved recipe. Studio then starts one fill run
(`POST /api/curation-fills`) with every column the chosen views read, each
once, and the views, records every `node → view name, revision` in one edit,
and saves once: a publication is recorded against the saved revision, and the
save that records it must come next.

Before the run starts, the backend checks each view name and admits every
statement as one read-only query. The run is the backend's and outlives the
page. For each column in
order it
reads the episodes that reach the node, skips those the column holds, asks
about the rest (a statement eight at a time; the Modal service twenty
across runs, its containers each taking in eight uploads while the GPU
scores, and
retries 429s, 5xx, and timeouts with a pause), and holds the answers in
memory; then it ingests them into a staging table, adds the column the first
time, merges it in by `episode_id`, and drops the staging table. A rejected
video (422) is that episode's failed answer; a refused token, a service on
another revision (409), or an error ends the run once what it answered is
written.
Once every column is filled it checks that each view's statement plans
(`LIMIT 0`) and stores it in the Studio SQLite table `curated_views`,
replacing the statement its name had, so a reader sees the old view or the
new one. Spiral keeps no hosted views, so Studio does: Data reads a view by
running its statement, and no other client reads it by name. A project bundle
carries none, since it carries no answers. A restarted backend loses the answers of the column in flight;
publishing again resumes past every episode already written. While a run
fills a view, Data lists it as "In progress", says which node and how many
of its episodes are answered, and opens it once the run is done. Unpublish
forgets the view; its stored answers stay.

A later save that changes the recipe moves the curation past the published
revision, and Publish then lists the output's view as out of date until it
is published again. Removing a published output's branch waits until it is
unpublished; a step may follow it, and it stays an output.

## Documents

Curations live in the Studio SQLite table `curations` and are saved only on an
explicit Save, with an optimistic revision check: the submitted revision must
equal the stored one, and the saved copy carries the next. A rename saves at
once, from the stored copy, so only the name moves and unsaved graph edits
stay unsaved. Any saved curation can be deleted from its row in the switcher, open or not:
Studio reads its stored copy, drops every view that copy publishes, and then
deletes it at the revision just read. A view that fails to drop stops
the delete. Deleting the open one opens a blank graph. The payload is
opaque to the backend and versioned by the frontend. Encoding 4 keeps only the
recipe: nodes, edges, each node's configuration and rule, and the published
outputs. Break down keeps its camera, sample interval, and the topic it
reads each episode's task from; Find similar its query and cutoff; Group
similar its categories and the ones it keeps; Score progress its threshold;
a Select its conditions. Nothing any node read is saved. A document in an
earlier encoding is refused.

## Why

| Decision | Why | Rejected |
| --- | --- | --- |
| No SQL node, joins, or free-form query shapes | Curation always ends in training input; analytics is not the product | A SQL escape-hatch node |
| Every node compiles to Spiral SQL, and a published view is the whole curation | One executable form: every reader sees exactly what was curated | Studio materializing results |
| Per-row model work is a `spql.ai` function, and progress scoring the Modal service; an authoring suggestion, such as proposed categories, stays in the browser and becomes a literal | A view reads only what Spiral stores | Browser-orchestrated calls per episode |
| Model answers are stored per dataset, one column per exact call, filled by a backend run | A view over the whole dataset must not ask its models on every read, and identical calls share their work | Re-asking on every read over the first 20 episodes; waiting for Spiral to cache |
| A preview runs only its node's step, over its parent's preview rows, reading its stored answers | Each step is written once, and a preview's answers are the ones a publish reuses | Recomputing the whole path per episode |
| Preview reads the first 100 episodes in recording order | Fast enough to iterate on | A random sample |
| Every Score progress, Find similar, Group similar, and Select node is an output; a measure keeps by its own rule; Publish chooses and names the views | Break down's free text is not trainable until a search or grouping reads it; a threshold belongs with the measure it divides | An explicit Output node; progress and similarity Selects |
| One row per kept interval | It is what the graph carries, so publishing converts nothing | One row per episode with nested ranges; one row per moment |
| Curation only chooses data | Sample rate, cameras, image size, and normalization are the policy's input contract, which rollout replays | Shape nodes in Curation |
| Publish is explicit and named; Save stays local | Exploratory leaves must not become views readers depend on, and readers need a stable name | Auto-publishing every leaf |
| The frontend compiles; the backend admits read-only statements, fills the columns they name, and stores the view | The backend stays thin and the browser never sends DDL | The backend compiles the graph |

## Not yet

- Columns no curation reads any more stay in the answers table; collecting
  them is later. Cancelling a running query is Spiral's to add.
- Training reads published views ([Training](training.md)): it resamples
  each kept interval onto its own clock, and the checkpoint carries how the
  model saw the data. Exposure normalization and colour jitter are not
  there yet.
- A run pins what it read: its uploads are named by the dataset version they
  were read from, though its record keeps only the view's name.
