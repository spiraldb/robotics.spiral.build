# Storage schema changelog

## Spiral schema 12

- Studio writes three kinds of Spiral resource at the project root: the
  `dataset_source` table (`dataset_name`, `recording`, and a `source` Blob of
  immutable MCAP bytes), one `dataset_source_<name>` view and native
  `dataset_<name>` Dataset per dataset, and `studio_training_checkpoints`.
- Each recording's MCAP header, each project bundle's data manifest, and each
  training job request carry this version.
- Training workers accept schemas 11–12.
