# Storage schema changelog

## Spiral schema 12

- Studio writes each dataset as plain tables under `datasets.<id>`:
  `episodes`, `topics`, one `messages.<stream>` table per recorded stream,
  `answers`, and for an MCAP dataset `files` (`recording` and a `source`
  `spql.io.Blob`). It decodes every MCAP file and LeRobot repository itself
  when it imports them, and makes or reads no Spiral Dataset, Location or
  view ([datasets](../../../../docs/datasets.md)). Recordings and project
  bundles kept their shape across that move, so it kept this number; before
  it, Studio wrote a `dataset_source` table, a `dataset_source_<name>` view
  and a native `dataset_<name>` Dataset per dataset.
- Each recording's MCAP header and each project bundle's data manifest carry
  this version.
