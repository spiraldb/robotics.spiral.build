# Studio catalog layout

The selected Spiral project contains Studio resources at its root:

| Resource | Kind | Purpose |
| --- | --- | --- |
| `studio_*` | Table | Robots, settings, feature registry, workflow, training, and rollout state |
| `dataset_source` | Table | All recording sources, with `dataset_name`, `recording`, and `source` columns |
| `dataset_source_<name>` | Ordinary view | Selects `recording, source` where `dataset_name = '<name>'` |
| `dataset_<name>` | Native Dataset | Uses the corresponding source view through `spql.mcap.dataset` |

The source views are logical queries. A Dataset refresh consumes its view's
selected rows and updates the Dataset's native population. Different datasets
can therefore share one source table without sharing their selected recordings.
The definition's `sourceRelation` records the association; ownership is not
inferred from the naming convention.

Studio imports create or append the shared table, create the filtered root view,
create and refresh the Dataset, and verify its episode count in one transaction.
Existing target resources or source rows with the same dataset name are refused.
The source table does not declare a primary key. `recording` is the native
producer's member key within each dataset's filtered input.

`spiral_robotics` and `spiral_dataset` were marker tables used as containers.
They are removed by this migration. Source views are at the root because the
current Spiral child-view path requires keyed parent metadata that Arbor does
not expose. Dataset-owned resources retain their native parent relationship.

The server's `--studio-table-prefix` defaults to `studio`; it is a naming prefix,
not a namespace. Storage schema 10 and training protocol 7 use this layout.
Rebuild/restart Studio and its training workers together after migration.

## Existing-project migration

`migrations/root_source_views.py` copies legacy source rows into the shared table
using native SQL, comparing all values including SHA-256 hashes of embedded Blob
bytes. Studio tables move through the project journal with their exact IDs and
versions. Dataset definitions receive immutable successors that use the root
views while retaining their logical member contents and private storage layout.
Publication moves the names, selects the Dataset successors, and removes legacy
source and container bindings in one compare-and-swap journal commit. Historical
commits retain the previous catalog and immutable resources.

Run Python 3.14+ with `pyarrow` and `adbc_driver_manager` and the current native
driver. Credentials default to Studio's local bootstrap file and never enter
the audit. Use a new audit directory for preparation:

```sh
python migrations/root_source_views.py prepare --audit /absolute/path/to/audit
python migrations/root_source_views.py publish --audit /absolute/path/to/audit
python migrations/root_source_views.py verify --audit /absolute/path/to/audit
```

The script refuses a changed catalog or a second publication attempt. After an
uncertain network result, inspect the saved request and journal before taking
further action. The `verify` phase compares Studio table contents, full episode
rows, source contents, resource identities, and the selected Dataset versions.

The September 8, 2026 audit is stored locally in
`exports/flat-catalog-root-views-2026-09-08/`. The earlier namespace migration
and obsolete HF demo removal are retained in their corresponding audit folders.
The saved default Workflow still refers to that previously removed HF demo;
this migration preserves that document unchanged.
