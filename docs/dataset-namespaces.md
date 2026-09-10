# Studio catalog layout

The selected Spiral project contains Studio resources at its root:

| Resource | Kind | Purpose |
| --- | --- | --- |
| `studio_*` | Table | Robots, settings, feature registry, workflow, training, and rollout state |
| `dataset_source` | Table | All managed MCAP sources, with `dataset_name`, `recording`, and `source` columns |
| `dataset_source_<name>` | Ordinary view | Selects `recording, source` where `dataset_name = '<name>'` |
| `dataset_<name>` | Native Dataset | Uses the corresponding source view through `spql.mcap.dataset` |

The source views are logical queries. A Dataset refresh consumes its view's
selected rows and updates the Dataset's native population. Different datasets
can therefore share one source table without sharing their selected recordings.
The definition's `sourceRelation` records the association; ownership is not
inferred from the naming convention.

Studio imports create or append the shared table, create the filtered root view,
create and refresh the Dataset, and verify its member count in one transaction.
Hugging Face import refuses existing target resources or source rows with the same dataset name. Live recording appends to a validated matching Dataset; an identical recording key and byte payload is an idempotent retry.
The source table does not declare a primary key. `recording` is an opaque source key within each dataset’s filtered input; it is
not a filename-derived dataset member identity.

`spiral_robotics` and `spiral_dataset` were marker tables used as containers.
They are no longer part of the catalog. Source views are at the root because the
current Spiral child-view path requires keyed parent metadata that Arbor does
not expose. Dataset-owned resources retain their native parent relationship.

The server's `--studio-table-prefix` defaults to `studio`; it is a naming prefix,
not a namespace. Storage schema 11 and training protocol 7 use this layout.
Rebuild/restart Studio and its training workers together after migration.

## Existing projects

Studio rejects unsupported storage layouts instead of recreating the retired
recording tables. One-off migration scripts have been retired; historical audit
receipts remain with their backups. Current project transfer uses the supported
`.spiral-project` format through Settings.

Live recording and project backup details are in
[MCAP recording and playback](recording-mcap.md). There is no separate recording
registry.

Capture and external import share these resources. Dataset readers do not depend
on this ingestion layout; see [Dataset and recording boundary](dataset-boundary.md).
