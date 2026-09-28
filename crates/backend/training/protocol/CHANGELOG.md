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
