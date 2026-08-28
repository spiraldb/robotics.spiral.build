# Storage schema changelog

## Spiral schema 8

- Replaced the `training_runs` JSON document with typed run rows, and added the
  `training_run_metrics` and `training_run_checkpoints` child relations.
- Replaced the `rollout_runs` JSON document with one typed row for the latest
  policy session, keyed by `session_id`.
- Added `document_version` to `workflow_runs` so a persisted Workflow or
  Assistant document declares the encoding of its payload.
- Removed the `episodes.metadata_json` document. Identity, timing, source,
  note, duration, follow error, and discarded state were already duplicated
  into columns and are now the only copy; recording gaps became the parallel
  `recording_gap_after_ns` and `recording_gap_duration_ns` lists on `episodes`;
  and the per-joint motor condition became typed lists on `episode_streams`
  (`start_temperature_celsius`, `start_voltage_volts`, the four `limit_*`
  channels, `peak_temperature_celsius`, `min_voltage_volts`, `peak_load`,
  `peak_current_amps`). `episode_streams` also gained `ordinal`, which fixes an
  episode's arm order instead of trusting physical row order.
- Pre-schema-8 projects are not readable. Studio detects their legacy relation
  shapes, leaves every row untouched, and tells the operator to start a fresh
  project.

## Spiral schema 7

- First recorded Spiral Robotics relational schema. Training, rollout, and
  Workflow history were single JSON documents keyed by `run_id`.
