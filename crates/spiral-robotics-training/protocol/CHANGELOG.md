# Training protocol changelog

## Training API 6.0

- Added the required `spiral_schema_version` job field.
- Added the stable API identifier, additive minor revision, and supported
  storage schema range to runtime health.
- Replaced exact integer compatibility with major/minor and schema-range rules.

## Training API 5.0

- Replaced optimizer sessions and Studio-proxied batches with worker-owned
  jobs, cursored telemetry, and direct checkpoint publication.
