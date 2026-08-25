# Ingestion

## Outcome

Ingestion brings a public Hugging Face dataset snapshot into the active
SpiralDB project as native, queryable Spiral relations. It is a producer peer
to [recording](../recording/README.md), but it does not rewrite external data
into Studio's SO-ARM101 episode schema. The imported dataset joins Data's
catalog and its relations are available to Query and Workflow.

**Status: implemented for public Hugging Face LeRobot and MCAP repositories.**

## Sources

- **LeRobot** requires at least one `data/**/*.parquet` file. Studio also
  discovers `meta/episodes/**/*.parquet`, `meta/tasks.parquet`, and
  `videos/**/*.mp4` when present.
- **MCAP** requires at least one `.mcap` file anywhere in the repository.
- **`.spiral-episodes` bundles** remain the separate, existing lossless episode
  transfer path in Data.

Private or gated Hugging Face repositories, local files, and object-store
prefixes are not supported by this ingestion path.

## User flow

1. The user opens Data and enters a new target dataset ID in the Dataset field.
2. The user chooses **Add data**, selects LeRobot or MCAP, and enters a public
   Hugging Face `namespace/repository` plus an optional revision.
3. Studio inspects the repository without changing Spiral. The inline preview
   shows the resolved commit, selected files, total known bytes, planned
   relations, and format-specific warnings.
4. The user confirms the immutable snapshot.
5. Spiral performs the remote reads, format parsing, and materialization in one
   catalog transaction.
6. Studio refreshes the dataset and relation catalogs and puts a useful starter
   query in Query. The import panel reports the commit and created relation
   count.

## Requirements

### Inspection and identity

- **INGEST-001** The browser must contact Hugging Face only through the Studio
  backend. The browser must not list repository files or read source bytes.
- **INGEST-002** Inspection must accept a Hugging Face dataset URL,
  `hf://datasets/...` URI, or exact `namespace/repository` identity.
- **INGEST-003** Inspection must resolve the requested revision to an immutable
  commit and must recursively discover only files relevant to the selected
  format.
- **INGEST-004** Repository discovery must stop after 100 pages or 50,000
  entries and must reject pagination links outside `huggingface.co`.
- **INGEST-005** Import must re-inspect the commit selected in the preview. It
  must not replace that commit with a moving branch or tag.
- **INGEST-006** The preview must identify the exact target dataset, selected
  files, known byte total, planned qualified relations, and warnings before any
  catalog mutation.

### Spiral boundary and integrity

- **INGEST-007** Studio may use the Hugging Face repository API for metadata
  discovery. Spiral must own every remote byte read, Parquet decode, MP4 index,
  and MCAP parse used for durable import.
- **INGEST-008** Import must create the dataset catalog row and every planned
  relation in one Spiral catalog transaction. A failed import must roll back
  instead of publishing a partially admitted dataset.
- **INGEST-009** Import must refuse an existing dataset ID or occupied relation
  prefix. It must never replace or merge an existing import implicitly.
- **INGEST-010** Dataset import must use the shared catalog-mutation lease and
  must be refused while recording, replay, rollout, training, Workflow, or
  another catalog mutation makes import unsafe.
- **INGEST-011** Relation names must be deterministic within the configured
  Studio namespace and derived from the target dataset ID.
- **INGEST-012** The dataset catalog description must preserve the source
  format, repository, and resolved commit.

### LeRobot materialization

- **INGEST-013** LeRobot frame Parquet files must materialize as one Arbor
  relation through `spql.parquet.read` over the commit-pinned Hugging Face URI.
- **INGEST-014** Episode metadata and task Parquet files must materialize as
  separate Arbor relations when present.
- **INGEST-015** MP4 files must be represented as external `spql.io.Blob`
  values and indexed by Spiral's MP4 Dataset producer. Frames, metadata, tasks,
  source blobs, and media assets must remain separately queryable relations.

### MCAP materialization

- **INGEST-016** MCAP files must be represented as external `spql.io.Blob`
  values and parsed by Spiral's MCAP Dataset producer.
- **INGEST-017** MCAP import must expose Spiral's structural relations,
  topic-specific relations discovered during parsing, and the quarantine
  relation. An unreadable file may be quarantined while other recordings
  import successfully.
- **INGEST-018** Studio must not claim decoded ROS, CDR, protobuf, or Foxglove
  message semantics when Spiral exposes only raw MCAP payloads.

### Catalog and query integration

- **INGEST-019** The Data dataset selector must include catalog datasets even
  when they contain no Studio episode rows.
- **INGEST-020** Query must discover current Spiral relations from the backend
  catalog rather than from only Studio's fixed managed-table list.
- **INGEST-021** A successful import must refresh both catalogs and install a
  format-appropriate read-only starter query without running it automatically.
- **INGEST-022** Hugging Face repositories, files, credentials, and import
  controls must remain outside Workflow. Workflow receives only the resulting
  Spiral relations as Table roots.

## Capability boundary

- Imported LeRobot and MCAP relations are queryable and use Data's existing
  schema-aware result table and media/image rendering where their Spiral types
  support it.
- Import does not normalize external rows into Studio episodes. Timeline,
  replay, episode-bundle export, and the current training preset therefore do
  not automatically apply to imported relations.
- Import is a single confirmation operation, not a resumable background job.
  There is no cancellation or per-file progress UI after confirmation.
- Hugging Face authentication is not configured in Studio; inspection and
  import currently support public repositories only.

## Implementation ownership

- Inline Data workflow and dynamic Query relation catalog:
  [`frontend/activities/data`](../../crates/spiral-robotics-frontend/src/activities/data/)
- HTTP inspection, import, and relation routes:
  [`backend/http/routes/data.rs`](../../crates/spiral-robotics-backend/src/http/routes/data.rs)
- Hugging Face discovery and Spiral materialization:
  [`storage/ingestion.rs`](../../crates/spiral-robotics-storage/src/ingestion.rs)
- Shared API contracts:
  [`spiral-robotics-api/src/datasets.rs`](../../crates/spiral-robotics-api/src/datasets.rs)
