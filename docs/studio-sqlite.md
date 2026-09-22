# Studio state in SQLite

The backend owns two persistence boundaries:

- **SQLite / SeaORM:** connection settings, robot documents and calibration,
  project preferences, saved workflows and conversations, saved curation
  recipes, Build documents, training history (including metrics and checkpoint
  references), and rollout state.
- **Spiral / ADBC:** datasets, recordings, MCAP payloads, query outputs and
  feature-cache values, and model checkpoint artifacts. The training worker
  continues to publish checkpoint bytes directly to Spiral.

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

`SPIRAL_ROBOTICS_BACKEND_STATE` selects the legacy JSON settings file to import;
it does not change the SQLite destination. That JSON file is only read when
the SQLite connection-settings row is absent.
Subsequent starts use SQLite, including saved credentials. New database files
use mode 0600 and new parent directories use mode 0700 on Unix.

One database contains all local projects. A project's key is its normalized
Spiral API endpoint plus project ID. Credentials and session IDs are excluded.
Different backends have independent Studio records even when they use the same
Spiral project. Records saved before selecting a project remain in the local
unconfigured-project scope.

SQLite uses WAL, full synchronization, foreign-key enforcement, and the
SQLite driver's busy timeout. Use SQLite's backup mechanism for a live backup;
copying only the main file while WAL is active can omit committed state. A
stopped backend's database can be copied after its connections have closed.

## Schema changes

SeaORM entities live in `crates/backend/storage/src/studio`. Product-facing
methods stay with their feature; callers do not receive ORM models. Robot and
workflow edits use conditional writes to preserve conflict detection. Training
history is stored per run, with storage-owned versioned JSON for its nested
configuration, metrics, checkpoint references, and logs. Non-finite metrics have
an explicit JSON encoding, so diverged losses survive a restart.

A Build document is stored as its own row: identity, name, revision, the
frontend's encoding version, and the payload as opaque TEXT. Storage never
parses that payload, so a change to the Build graph's schema needs no migration.
Saves are conditional on the revision the document was loaded at.

Checked-in migration scripts live in `studio/migrations`. Add a new, uniquely
named migration and register it in `Migrator`; do not edit an applied migration.
Startup applies pending migrations **before accepting HTTP requests**. The
migration operations and SeaORM migration journal share a SQLite transaction;
a failure aborts startup. Schema downgrades require restoring a backup.

The SQLite migration sequence is independent of
`crates/backend/storage/schema/version.json`. That version remains the Spiral
data/artifact contract used by the training worker. Project bundles have their
own format version.

## Retired Spiral control tables

Control records have moved to SQLite. Runtime reads never fall back to Spiral,
and the one-time migration command has been removed. Original control tables
may remain in Spiral as a safety copy; the backend filters their exact configured
names out of catalog responses. `dataset_source`, its views, model artifacts,
and user-created tables remain visible as data. The Data tree has no
Studio-internals section.

## Project transfer

Version **3** `.spiral-project` bundles are ZIP archives containing exactly:

- `studio.sqlite3`: a native SQLite snapshot of the selected project's Studio
  records. SQLite's `VACUUM INTO` captures committed state, including WAL data.
  The export removes backend settings, credentials, and other projects, then
  vacuums again to remove their deleted bytes. Project keys are neutralized so
  the snapshot can be restored into another project.
- `spiral.data`: a versioned manifest and Arrow streams containing MCAP source
  files and training checkpoint artifacts. Arrow is used only for data-plane
  payloads; Studio records have no Arrow backup schemas.

Older bundle formats are unsupported. SQLite snapshots must match the current
managed schema. Imports open snapshots read-only, validate their integrity and
records, and insert records through local ORM entities. They never execute
uploaded schema or migration scripts. The archive and its total uncompressed
contents are limited to 256 MiB; Arrow decoding has additional allocation bounds.

Import validates the bundle and stages local replacements, including constraint
checks, in a SQLite transaction before mutating Spiral. Spiral's data transaction
commits first, then SQLite commits. A Spiral failure rolls back staged local
state. If Spiral commits but the final SQLite commit fails, the error explicitly
reports partial completion and instructs the operator to retry the same import.
There is no distributed transaction across these two databases. Project admission
continues to exclude active work during import and prevents stale requests from
being retargeted to another project.

## Availability and verification

Studio readiness is independent of a configured or reachable Spiral destination.
Robot setup, preferences, saved workflows, and history load locally. Data queries,
recording publication, training data access, and artifact reads still require
Spiral and report their own connection errors. Restart recovery never resumes
interrupted robot motion.

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
