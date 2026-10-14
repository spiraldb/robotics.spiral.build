# Studio state in SQLite

The backend owns two persistence boundaries:

- **SQLite / SeaORM:** each browser's Spiral session (its application OAuth tokens and
  chosen workspace), the workspaces and their robot connector tokens, the
  episodes those connectors uploaded until a browser stores them, each dataset's
  robot model and joint roles, saved
  curation recipes and the statements of the views they publish, and a record
  of each training run Studio started (its configuration and what it was
  given).
- **Spiral / ADBC:** datasets, their MCAP sources, and each dataset's answers
  table: model answers and the health check's columns, one cell per episode.
  Training
  runs' metrics, logs, and checkpoints live on the Modal training service
  ([Training](training.md)).

Robot documents and calibration are not Studio state: the robot connector that
drives the arms saves them, in its own data directory.

## Location and lifetime

The default database is `.studio/studio.sqlite3` in the repository used to build
the backend. The entire `.studio/` directory is Git-ignored, including SQLite's
WAL and shared-memory files. The path is anchored to the source repository, so
launching from a different working directory still uses the same database.
The backend prints the resolved path in its startup log.

Set `SPIRAL_ROBOTICS_STUDIO_DB` to an explicit file path to override it. Use
this override when deploying the executable outside its source checkout, and
keep that directory on a persistent volume in a container. Backend reloads
and rebuilds in the same checkout retain the database.

New database files use mode 0600 and new parent directories use mode 0700 on Unix.

One database contains all local projects. A project's key is its normalized
Spiral API endpoint plus project ID. Credentials and session IDs are excluded.
The `workspaces` table names projects: one row per endpoint and project, with
the name every user sees. A project's records keep the project key, so renaming
a workspace moves nothing; deleting one deletes its project's records with it.
Projects that held records when the table was added became workspaces named by
their project ID.
The `sessions` table holds each browser's refresh token, so the database file
is as sensitive as a signed-in session; project bundles never include it.
Different backends have independent Studio records even when they use the same
Spiral project. Records saved before selecting a project remain in the local
unconfigured-project scope.

SQLite uses WAL, full synchronization, foreign-key enforcement, and the
SQLite driver's busy timeout. Use SQLite's backup mechanism for a live backup;
copying only the main file while WAL is active can omit committed state. A
stopped backend's database can be copied after its connections have closed.

## Schema changes

SeaORM entities live in `crates/backend/storage/src/studio`. Product-facing
methods stay with their feature; callers do not receive ORM models. Curation
edits use conditional writes to preserve conflict detection. A
training run is stored as its ID beside the JSON of what Studio asked it to
do.

A curation is stored as its own row: identity, name, revision, the frontend's
encoding version, and the payload as opaque TEXT. Storage never parses that
payload, so a change to the curation's schema needs no migration. Saves are
conditional on the revision the document was loaded at.

Checked-in migration scripts live in `studio/migrations`, starting from one
baseline that creates the whole schema. Add a new, uniquely named migration and
register it in `Migrator`; do not edit an applied migration. Startup applies
pending migrations **before accepting HTTP requests**. The migration operations
and SeaORM migration journal share a SQLite transaction; a failure aborts
startup. Schema downgrades require restoring a backup.

The baseline, `m20260928_000001_studio`, replaced every earlier migration, so a
database created before it cannot be upgraded. Startup then stops with
`error: Studio database <path>: … Migration file of version '<old name>' is
missing …`, one line per old migration: delete `.studio/studio.sqlite3` (and its
`-wal` and `-shm` files) and start again. There is nothing to export first, because bundles
exported before it cannot be imported.

The SQLite migration sequence is independent of
`crates/backend/storage/schema/version.json`. That version remains the Spiral
data contract recordings carry. Project bundles have their own format version.

## Leftover Spiral control tables

Studio keeps no control records in Spiral. A project older than the move to
SQLite may still hold the retired `studio_robots`, `studio_settings`,
`studio_training_runs`, `studio_training_run_metrics`,
`studio_training_run_checkpoints`, `studio_training_checkpoints`,
`studio_rollout_runs`, `studio_workflows`, and `studio_feature_registry`
tables, which the Data catalog lists like any other table. Drop them in
Spiral; nothing reads them.

## Project transfer

Version **5** `.spiral-project` bundles are ZIP archives containing:

- `studio.sqlite3`: a native SQLite snapshot of the selected project's Studio
  records. SQLite's `VACUUM INTO` captures committed state, including WAL data.
  The export removes Spiral sessions, workspaces, waiting episodes, published curation views
  (a bundle carries no model answers for them to read), and other projects,
  then vacuums again to remove their deleted bytes. Project keys are neutralized so
  the snapshot can be restored into another project.
- `manifest.json`: `{"format":"spiral-robotics-project","format_version":5,
  "schema_version":<Spiral schema>,"mcap_datasets":[…]}`; unknown fields are
  refused.
- `dataset_source.arrows`: one Arrow IPC stream of every MCAP dataset's
  files (`dataset_name`, `recording`, and `payload`, the MCAP bytes, read
  back whole from wherever each file's Blob points). LeRobot datasets are not
  carried; import one again from Hugging Face.

A relation absent from Spiral is exported as an empty stream with its canonical
schema. Arrow is used only for data-plane payloads; Studio records have no Arrow
backup schemas. Import requires exactly these three entries. A ZIP without
`manifest.json`, which includes every version-3 bundle, fails with "Not a
format 5 project; project files from older Studio versions cannot be imported";
any other `format_version` or Spiral schema version is refused. SQLite
snapshots must match the current managed schema. Imports open snapshots
read-only, validate their integrity and records, and insert records through
local ORM entities. They never execute uploaded schema or migration scripts.
The archive and its total uncompressed contents are limited to 256 MiB; Arrow
decoding has additional allocation bounds.

Import validates the bundle and stages local replacements, including constraint
checks, in a SQLite transaction before mutating Spiral. One Spiral transaction
then drops every table of the project's MCAP datasets and writes each dataset
the manifest names again from the bundle's files, decoding them as any MCAP
import does ([datasets](datasets.md#writing)); a dataset without files is
made empty, and every restored episode gets a new identity. The project's
LeRobot datasets stay as they are, so a bundle naming a dataset the project
holds as anything but an MCAP dataset is refused before anything changes.
Spiral's data transaction commits first, then SQLite commits. A Spiral failure rolls back staged local
state. If Spiral commits but the final SQLite commit fails, the error explicitly
reports partial completion and instructs the operator to retry the same import.
There is no distributed transaction across these two databases. Importing a
project refuses while it has active work, and replaces its open project so no
cache of the replaced data survives.

## Availability and verification

Studio features are independent of a chosen or reachable Spiral project.
Curations and training run records load locally. Data queries,
recording storage, training data access, and artifact reads still require
Spiral and report their own connection errors.

Run the storage and backend tests with the configured local ADBC driver:

```sh
nix develop .#local-driver --command cargo test \
  -p spiral-robotics-storage -p spiral-robotics-backend --lib --locked
```

SQLite tests use isolated temporary files or in-memory databases. They cover
restart persistence, migration rollback, project isolation, revision conflicts,
failed writes, snapshot privacy and portability, local restore atomicity,
Spiral failure rollback, bundle-version rejection, and operation with Spiral
unavailable.
Hosted data-transfer tests still require an explicitly selected disposable
Spiral project.
