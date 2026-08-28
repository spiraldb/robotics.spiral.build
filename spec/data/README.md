# Data

## Outcome

Data is the center of the product: the place where the project's episodes
live and where the user spends their time viewing, querying, curating,
replaying, importing, exporting, and deleting them. Producers fill it —
[recording](../recording/README.md) and [ingestion](ingestion.md) — and
consumers draw from it: query, curation, [training](../training/README.md),
[rollout](../rollout/README.md), replay, and export.

## Feature map

- Episode catalog and dataset operations are defined here.
- [Ingestion](ingestion.md) — public Hugging Face LeRobot and MCAP producers
- [Timeline and replay](timeline-and-replay.md)
- [Project data relations](relations.md)
- [SQL query](query.md)
- [SpiralDB persistence and schema](persistence.md)

## User flow

1. The user opens Data and selects a dataset.
2. Studio lists its episodes in episode order.
3. The user selects an episode to inspect or replay.
4. The user may switch to Relations to see the project's user-facing tables,
   exact row counts, shared identity fields, schemas, and bounded row previews.
5. The user may switch to Query without changing the route layout.
6. The user may import or export the selected dataset's episodes as one
   portable episode file. Whole-project backup and restore live in Settings.
7. The user may inspect and import a public Hugging Face LeRobot or MCAP
   snapshot as native Spiral relations, then query those relations in place.

## Datasets and episodes

- **DATA-001** One dataset must contain many ordered episodes.
- **DATA-002** The episode list must remain ordered by episode, not grouped by
  persistence or publication state.
- **DATA-003** Each episode row must offer selection, replay, and an icon-only
  delete action without requiring selection first.
- **DATA-004** A SpiralDB read or write failure must be visible without
  fabricating a local episode state.
- **DATA-005** Episodes and Query must be top-level panels in the Data route.
- **DATA-006** Episode import and export belong to the Episodes panel and are
  scoped to the selected dataset.
- **DATA-007** Deleting an episode must delete the episode and its owned streams
  as one operation.

## Import and export

- **DATA-008** Import must accept the versioned `.spiral-episodes` bundle.
- **DATA-009** Import must validate the complete candidate before adding
  durable rows.
- **DATA-010** A failed import must not leave partial rows or a quarantine area.
- **DATA-011** Export must produce one file containing the selected dataset
  metadata and all of its episodes, streams, samples, and media. It must not
  include robots, settings, training runs, checkpoints, or rollout state.
- **DATA-012** Import and export must preserve the complete relational and
  binary episode data needed for inspection, training, and replay.

## Catalog refresh

- **DATA-013** The Data Activity Controller must load the episode catalog when
  Data first opens, retain it across route changes, and update it from
  successful recording, deletion, and import responses. Studio-wide shell
  state must not own or eagerly load this catalog.
- **DATA-014** The episode catalog must not travel in the live event stream or
  use interval polling. The Episodes panel must provide an explicit Refresh
  action that reloads episodes and catalog issues in one request.
- **DATA-015** Studio must read the catalog directly from SpiralDB and must not
  poll a filesystem or maintain filesystem stamps for change detection.
- **DATA-016** Episode import must merge into the selected target dataset,
  retarget the imported episode rows to it, and refuse any episode-ID collision
  instead of overwriting an existing episode.
- **DATA-017** The Dataset field must accept a new dataset ID as well as list
  catalog datasets, including imported datasets with no Studio episode rows.
- **DATA-018** Add data must open the Hugging Face inspect-and-confirm workflow
  inline in Episodes and must not obscure Query with a modal.
- **DATA-019** Relations must expose the active project's user-facing table
  structure, counts, likely shared-key joins, schemas, and bounded previews
  without exposing Studio persistence plumbing as project data. Qualified
  names and schemas come from the startup catalog resource in `Studio` state;
  exact counts come from one Relations-view query and previews remain per
  selection.
- **DATA-020** Each catalog entry must pair its lightweight `EpisodeInfo` with
  an optional successful immutable full-data value. The selected episode must
  drive one reactive resource that owns transient loading and failure state,
  reads the entry's cached value when present, and otherwise loads and attaches
  the result to that same entry. Scene, Cameras, Signals, Timeline, scrubbing,
  and visual playback must derive from that selected resource and attached
  value rather than a second Data cache.
- **DATA-021** Storage must call the loaded header, samples, and recording
  summary aggregate `FullEpisode`; a persisted pause checkpoint must not be
  described as a completed episode.

## Curation direction

Confirmed direction, not yet specified in requirements: the user must
eventually be able to materialize a query result or a curated selection as a
new derived dataset, so training sets are shaped inside the Studio instead of
external scripts. Requirements will be added when this feature is designed.

## Feature relationships

- [Recording](../recording/README.md) creates episodes with connected arms.
- [Ingestion](ingestion.md) creates native Spiral relations from public
  Hugging Face sources. It does not imply Studio episode normalization.
- [Training](../training/README.md) consumes a dataset and creates runs and
  checkpoints.
- [Rollout](../rollout/README.md) consumes a checkpoint and records rollout
  evidence.

## Implementation ownership

- Data activity:
  [`frontend/activities/data`](../../crates/spiral-robotics-frontend/src/activities/data/)
- Episode import/export and the shared project store:
  [`spiral-robotics-storage`](../../crates/spiral-robotics-storage/src/)
- Dataset and episode use cases:
  [`backend/data`](../../crates/spiral-robotics-backend/src/data/)
- Stable relation names:
  [`spiral-robotics-storage/src/schema.rs`](../../crates/spiral-robotics-storage/src/schema.rs)
