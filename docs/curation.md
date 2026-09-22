# Curation

Curation prepares a dataset's episodes for training. A curation is a saved
recipe over one dataset: what to measure about each episode, which rules
exclude an episode, and which transforms the kept episodes receive. Running
the recipe judges every episode with one read-only Spiral query and lays the
verdicts out on a contact sheet: kept episodes in full ink, excluded episodes
dimmed under the rule that removed them.

Curation is a specialization of Workflow. Workflow authors arbitrary SQL over
relations; Curation compiles a typed recipe to SQL whose input and output are
episodes. Both run through the same native executor, the same read-only
policy, the same document revisions, and the same awaited runs.

## The recipe

- **Source.** One dataset, addressed by its catalog name. The population is
  `spql.mcap.episodes('<dataset>')`; nothing is copied or changed.
- **Measurements.** Every episode already carries five facts: duration in
  seconds, message count, channel count, file size in megabytes, and source
  path. A recipe may declare more, each with a lowercase key rules refer to:
  - *Camera brightness*: mean relative luminance of sampled frames of one
    camera topic, decoded natively from the recorded packets
    (`spql.video.from_packets` → `spql.video.frames` with explicit frame
    indices → `spql.video.decode` → `spql.image.mean_luminance`).
  - *Camera score*: `spql.ai.score` of the sampled frames against a prompt.
  - *Topic field*: one message field folded per episode (first, last,
    minimum, maximum, average, or count), such as a recorded success flag;
    declared as a number or text.
  - *SQL*: any read-only `SELECT`/`WITH` returning `id` and `value` per
    episode, admitted by the same policy as Workflow SQL.
- **Rules.** `If <fact> <operator> <value> then exclude as <reason>`. Number
  facts compare with below / at most / above / at least / is / is not; text
  facts with contains / does not contain (case-insensitive) / is / is not.
  Either kind may test is missing / is present. A rule whose fact is NULL
  does not fire unless it tests for missing. Every firing rule is named on
  the episode it removed; a rule may be kept in the recipe but switched off.
- **Transforms.** Camera colour profile (`spql.image.to_rgb` assumption),
  camera resize, or a trim of episode time, applied to every kept episode
  or only to the kept episodes a condition picks. Transforms are recorded
  with the recipe and counted on the sheet; applying them requires
  materializing the kept episodes as a dataset, which Studio does not do yet.

Validation names each problem with its way out: a missing dataset, an
unknown fact, an operator that does not fit the fact's kind, a comparison
without a value, a SQL measurement that is not read-only. A recipe with
errors never becomes a run.

## Compilation

`CurationSpec::compile` in `spiral-robotics-common` produces two statements
from one `WITH` chain: `episodes` (the population with its statistics), one
CTE group per declared fact yielding `(id, value)`, `facts` (episodes left
joined to every fact), `judged` (every enabled rule and transform as a
boolean column), and `verdicts` (`kept = NOT (COALESCE(rule_0, false) OR …)`).
Facts, rules, and transforms use positional aliases (`fact_0`, `rule_0`,
`transform_0`), so identities never enter SQL; the compiled statement maps
them back when reading rows. Literals travel through one quoting path.

The **roster** statement lists one row per episode ordered by recording
time and is bounded by the query preview limit (1,000 rows). When Spiral
truncates it, the **totals** statement runs the same chain again to count
episodes, kept episodes, and each rule's and transform's population exactly.
The sheet then says it is truncated while the counts stay whole.

## Runs

`POST /api/curation-runs` compiles the submitted recipe, unsaved edits
included, judges it, and answers when the run ends. The run holds the exact
recipe, the compiled SQL, a phase (running, complete, failed), and on
completion an outcome: totals, per-rule and per-transform counts, and one
verdict per listed episode with its statistics, fact values, the rules that
fired, and the transforms it would receive. Runs are not persisted: the
browser shows a running placeholder while it awaits the request and keeps the
answered runs until the page reloads. Starting a run takes the catalog lease
so it cannot overlap recording or import.

Verdicts stay current while the judged recipe definition is unchanged;
renaming or saving does not invalidate them, any recipe edit marks them as a
previous run.

## Documents

Recipes live in the Studio SQLite table `curations` (project, id, name,
revision, JSON document, updated time), saved only on explicit Save with the
same optimistic revision check as workflows: the submitted revision must
equal the stored one, and the saved copy carries the next. The stored
document is versioned independently of the wire type; an unsupported
version is refused and preserved. Project bundles include the table, and a
restored snapshot validates each row before staging it.

## The Studio surface

Curation is the fourth primary activity. Its default layout keeps the
**Recipe** in a 26% column at the left and the **Sheet** at the right, both
dockable workbench panels. The selected episode's evidence is not a third
panel: it docks inside the Sheet as a side sheet, so nothing sits empty at
rest.

- The document bar names the curation and its saved state and holds
  Discard, Save, and Run. Run stays enabled while the recipe is merely
  incomplete; pressing it then says what is missing instead of hiding the
  reason in a tooltip. ⌘/Ctrl+Enter runs from anywhere in the recipe.
- The Recipe's Source section lists catalog datasets (or accepts a typed
  catalog name when the list is unavailable) and reports the episode count
  and recorded time of the chosen one. Measurements rest as one line each,
  the name and what tells it from another of its kind; the five builtin
  facts are offered inside every rule's fact list under "Every episode".
  Rules read as two lines: *If ⟨fact⟩ ⟨op⟩ ⟨value⟩* and *exclude as
  ⟨reason⟩*. The mark leading a rule is its handle: hovering it lights the
  tiles the rule struck, pressing it pauses the rule. Transforms rest the
  same way as measurements.
- The Sheet's run strip reads the phase, an All / Kept / Excluded filter
  whose segments carry the totals, a lens naming the measurement every tile
  prints (with a dataset-order / lowest-first / highest-first toggle so the
  episodes near a threshold sit together), Spiral's judging time, a run
  selector once there is more than one run, and the executed SQL. Tiles
  show the ordinal, the distinguishing part of the source path on up to two
  lines, the duration as a 2px baseline against the longest episode, the
  lensed value, and, when excluded, the reasons in the removing rule's tint.
- Opening a tile docks the episode's evidence beside the sheet: its
  statistics, every fact value, each rule's verdict, each transform's reach,
  and, for camera measurements, the sampled frames decoded immediately
  through `POST /api/queries/episode`, which binds the episode UUID as an
  Arrow parameter (`$1`) because Spiral compares member ids only against
  bound UUIDs. ↑ and ↓ step through the tiles on view; Escape returns focus
  to the tile.

Rule tints are the six result-category washes dealt in recipe order. They
carry identity, never health, and every chip prints the rule's words.
Renaming the curation, a measurement, or a rule's reason never invalidates
the visible run; any edit that could change a verdict does.

## Limits

- Kept episodes are not yet materialized as a derived dataset; that remains
  the committed direction. The compiled SQL is shown so the selection can be
  reused in Workflow or Data.
- `spql.image.resize` requires non-null `u32` literals that SQL cannot spell
  from Studio today, so the resize transform is recorded but has no preview.
- Dataset and camera discovery use the existing catalog endpoints; when they
  fail, the recipe accepts a typed dataset name and free-text topic.
