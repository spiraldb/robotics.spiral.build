# Training protocol changelog

## Training API 8.0

- Job requests carry Studio's run configuration unchanged under `config`,
  beside `job_id`, `spiral`, and `spiral_schema_version`.
- `GET /v1/jobs` lists every retained job's state; job state no longer carries
  `finished_ms` or `resolved`.
- `GET /v1/jobs/{id}` returns the job state with its retained metrics and log
  lines. Replay cursors, `history_gap`, and snapshot checkpoints are removed:
  checkpoints are listed from Spiral.
- Removed `GET /v1/jobs/{id}/telemetry`.

## Training API 7.0

- Replaced `spiral.namespace` with `spiral.table_prefix`.
- Workers query flat `<prefix>_video_assets` and publish checkpoints to
  `<prefix>_training_checkpoints`; storage schema 10 is required.

## Training API 6.0

- Added the required `spiral_schema_version` job field.
- Added the stable API identifier, additive minor revision, and supported
  storage schema range to runtime health.
- Replaced exact integer compatibility with major/minor and schema-range rules.
- Extended the supported storage schema range through schema 9; the training
  relations and query contract are unchanged by Workflow relation namespacing.

## Training API 5.0

- Replaced optimizer sessions and Studio-proxied batches with worker-owned
  jobs, cursored telemetry, and direct checkpoint publication.
