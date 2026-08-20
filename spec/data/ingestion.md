# Ingestion

## Outcome

Ingestion brings externally produced robot data into the active SpiralDB
project as ordinary datasets and episodes. It is a producer peer to
[recording](../recording/README.md): for a user who arrives with a pile of
existing files, it is the primary way data enters the Studio.

**Status: confirmed direction, not implemented.** Every requirement below is
accepted product intent. No ingestion code, wire endpoint, or UI exists yet;
see **Current gaps**.

## Sources

Ingestion supports multiple source kinds behind one job model:

- **LeRobot datasets** — from Hugging Face or a compatible local/remote
  layout. First because their fixed layout (parquet, MP4, metadata) maps
  nearly 1:1 onto the episode schema already used for training.
- **MCAP files** — from an object-store prefix or local upload. MCAP is a
  container, not a schema, so MCAP ingestion includes a channel-to-stream
  mapping step. The long-term goal is that any well-formed MCAP file ingests
  without manual schema work; mapping assistance may improve incrementally.
- **`.spiral-episodes` bundles** — the existing lossless transfer format,
  already shipped as Data import and unchanged by this feature.
- **Recording with connected arms** — the built-in producer, specified in
  [Recording](../recording/README.md). Ingestion UI may link to it as a
  source, but it keeps its own flow.

## User flow

1. The user opens Data and chooses Add data.
2. The user picks a source kind and identifies the source: a dataset id, an
   object-store prefix, or local files.
3. The backend inspects the source and previews what would be ingested:
   episodes, streams or channels, sizes, and any mapping decisions needed.
4. For MCAP sources, the user reviews the channel-to-stream mapping and
   resolves anything the backend could not map automatically.
5. The user confirms, and the backend runs one ingestion job with visible
   per-episode progress.
6. Completed episodes appear in the target dataset in the episode catalog;
   the job report states what was ingested, skipped, and why.

## Requirements

### Job model and boundary

- **INGEST-001** Ingestion must run as a backend-owned job. The browser
  requests, observes, and cancels jobs; it must never parse source files or
  contact external services directly.
- **INGEST-002** External-source credentials (Hugging Face tokens,
  object-store keys) must be stored and used by the backend only, configured
  in Settings beside the existing service endpoints.
- **INGEST-003** Ingested episodes must land in the one active SpiralDB
  project as ordinary datasets and episodes. Ingestion must not create a
  second catalog, staging store, or shadow filesystem spool.
- **INGEST-004** An ingestion job must be cancellable. Cancellation must keep
  completely committed episodes and must not leave partial rows for the
  episode in flight.
- **INGEST-005** A job must report per-episode progress and errors. One bad
  source file must fail that episode with a stated reason, not the whole job.

### Validation and integrity

- **INGEST-006** Each candidate episode must be validated as one complete
  unit before any durable rows are written, matching the import rules
  **DATA-009** and **DATA-010**.
- **INGEST-007** Re-running ingestion over the same source must not silently
  duplicate episodes. Collisions with already-ingested source identities must
  be reported and skipped or explicitly replaced, never doubled.
- **INGEST-008** Every ingested episode must record its provenance: source
  kind, source location, original identity within the source, and ingestion
  time.

### Data shape

- **INGEST-009** Ingested channels must land as natural streams on one
  episode clock, following the existing stream model, not as one resampled
  wide table.
- **INGEST-010** An episode must not require the SO-ARM101 shape. Robot
  bindings, follow error, and replay compatibility must be optional: an
  episode without a matching configured robot must remain fully viewable,
  queryable, exportable, and usable for curation.
- **INGEST-011** Replay, physical seek, and training compatibility are
  conditional capabilities. Studio must state why an episode lacks a
  capability instead of hiding the affected actions.
- **INGEST-012** MCAP channels that do not map to a known stream kind must be
  preserved as generic signal streams rather than dropped, so no source data
  is lost at ingestion time.
- **INGEST-013** Video content must be converted or repackaged into the
  managed browser-playable form used by recorded episodes, with stream
  identity and clock offsets preserved.

## Feature relationships

- Ingested episodes join the [episode catalog](README.md) and are inspected
  through [Timeline and replay](timeline-and-replay.md) and
  [SQL query](query.md).
- [Training](../training/README.md) consumes ingested datasets exactly as it
  consumes recorded ones, subject to the checkpoint data contract.
- Durable writes follow [Data persistence](persistence.md).

## Current gaps

- Nothing in this document is implemented. There is no ingestion job, wire
  endpoint, source adapter, mapping UI, provenance column, or generalized
  episode schema in the code today.
- The current episode schema assumes recorded SO-ARM101 data (declared arms,
  ordered joints, follow error). Generalizing it per **INGEST-010** is a
  prerequisite for every non-LeRobot source and likely for LeRobot sources
  recorded on other hardware.
