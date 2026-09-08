# Studio data namespaces

Studio application state lives under `spiral_robotics`: robot configuration,
settings, feature registry, and workflow, training, and rollout records.

Native datasets and their source tables live under `spiral_dataset`. Imports
create `spiral_dataset.<name>` and `spiral_dataset.<name>_sources`; the namespace
is created in the same import transaction when absent. The dataset definition's
`sourceRelation` records the association. Names are not used to infer ownership.

## Existing-project migration

`migrations/dataset_namespace.py` uses Spiral's native resource versions and
project-journal namespace publication. It does not copy table rows, re-upload
recordings, rebuild video indexes, or convert the preserved HF demo format.

The migration moves the root `default` dataset and its source, renaming that
source to `spiral_dataset.default_sources`. The old `spiral_robotics.hf_demo_*`
objects move to `spiral_dataset` with their existing leaf names. Child resources
follow their stable parent's identity automatically.

Dataset definitions receive successor versions with corrected source paths.
For the MCAP dataset, the same logical member contents and private storage
backing are retained through the native Dataset revision API. The project
selects these successors and all path changes atomically. Dataset IDs and all
table IDs/versions are preserved. Historical project commits retain old paths.

Run using Python 3.14+ with `pyarrow` and `adbc_driver_manager` installed, and the
current native driver. The default mode is read-only:

```sh
python migrations/dataset_namespace.py \
  --project PROJECT_ID --audit /absolute/path/to/audit
```

`--apply` prepares and publishes the migration. `--verify` checks the published
catalog and counts against the audit. `--publish` replays only the saved
idempotent publication request after its outcome has been inspected; it does
not prepare duplicate Dataset versions. The script refuses to rerun preparation
when an earlier publication request exists. Credentials are read from Studio's
local bootstrap file and are not included in the audit.

The September 8, 2026 migration audit for the current project is stored locally
in `exports/dataset-namespace-2026-09-08/`. The catalog publication is
`01a08344-7a09-7370-bc60-9efbca01fa77`.

After migration, the obsolete HF demo was removed at the user's request. Its
six resources (episodes, frames, tasks, video Dataset, video assets, and video
source table) were removed together through the native journal. Every retained
resource and the default Dataset's exact contents were verified unchanged.
The current `spiral_dataset` namespace contains only `default` and
`default_sources`. The cleanup audit is in `exports/remove-hf-demo-2026-09-08/`;
previous journal commits retain the historical HF objects.
