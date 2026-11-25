# Spiral data-plane database boundary

Studio control-plane records use [local SQLite with SeaORM](studio-sqlite.md).
The boundary described below serves datasets, queries, and ingestion.

Storage features depend on `Database`, `DatabaseSession`,
`DatabaseConnection`, and `DatabaseTransaction`. `ProjectStore` receives an
`Arc<dyn Database>`; startup selects `SpiralSdkDatabase`, and tests
`MemoryDatabase`.

## Spiral's SDK

`SpiralSdkDatabase` reaches Spiral through the published `spiral-sdk` (library
`spiral`) and `spiral-datafusion` releases pinned together in `Cargo.toml`.

The SDK reads and writes through libspiral, linked into Studio from the release's
prebuilt static archive. `scripts/setup-spiral-sdk.sh` runs the SDK's packaged
setup script, which downloads the public archive, verifies its SHA-256 and ABI,
and caches it under `~/.cache/spiral/native/<source-sha>/<target>`.
`SPIRAL_NATIVE_CACHE_DIR` overrides the cache root. The default Nix shell runs
setup on entry; outside it, run:

```sh
scripts/setup-spiral-sdk.sh
```

CI's setup exports `SPIRAL_NATIVE_DIR`; the hosted image builds with
`nix build --impure` to read the archive from that directory. No private
repository credentials are required for the published SDK or runtime.

To move to another release, change both dependencies' versions in `Cargo.toml`,
update `Cargo.lock` and run `scripts/setup-spiral-sdk.sh`.

Every request authenticates with the signed-in user's token as Studio holds it
when the request is made, so long work outlives each short-lived token.

## Tables, statements and writes

Every table is a plain Spiral table at the project's root, named by dotted
parts as one name (`datasets.<id>.episodes`; see [datasets](datasets.md)); a
statement quotes the whole name as one identifier. `RelationName` holds such a
name.

- **Statements** are DataFusion SQL, planned by `spiral-datafusion` over the
  project's tables on `main` as committed when the statement starts, or, in a
  transaction, when the transaction began. Studio
  uses DataFusion's own functions and no SPQL ones. Parameters bind as `$1`
  onwards; a plain value binds without metadata, so Spiral filters its scan
  by it and skips what its statistics exclude. Results stream as they are computed and outlive their connection; a
  connection holds any number at once.
- **Writes** publish through SDK transactions. An autocommit `ingest` or
  `drop_table` is one commit. In a transaction, ingestion and drops are held
  and each table gets one write folded from them when it commits (appends
  extend each other, a replace or a drop supersedes what came before, rows
  written after a drop make the table again), since Spiral publishes one
  change per table in a transaction. Schema inspection in a transaction sees
  what it holds before what is committed; statements see only what is
  committed.
- An autocommit write pins `main` when it starts; a transaction pins it when
  it begins. Spiral validates the original versions of the tables it writes
  and the names it binds, publishing all changes atomically or returning
  `Conflict`. Episode numbering and topic counts read tables they also write,
  so competing writers cannot publish stale values. Unrelated table changes
  may commit concurrently. Reads of other tables are not automatically checked
  at commit. Studio serializes writers within one session's project
  (`connect_exclusive`); other sessions and processes use Spiral's conflict checks.

The SDK keeps no disk cache: every read fetches its data from SPFS. Reading one
episode's first 30 camera packets takes about 3–5 s each time, where the ADBC
driver it replaced took 4 s once and 0.12 s from its cache after.

## Verification

`MemoryDatabase` runs the same SQL on DataFusion's in-memory tables, every
connection sharing them, with transactions held and folded as on Spiral; the
storage and backend suites use it, and `the_in_memory_database_keeps_the_contract`
checks it against the contract every database keeps. The same contract runs
against a hosted project, creating and dropping two uniquely named tables:

```sh
SPIRAL_MCAP_TEST_PROJECT=<project> SPIRAL_ACCESS_TOKEN=<token> \
  nix develop --command cargo test -p spiral-robotics-storage \
  a_hosted_spiral_project_keeps_the_contract -- --ignored
```

A user token such as `spiral login --show-token` prints lasts about five
minutes.

## Startup and schema checks

SQLite opens and migrates before the HTTP listener starts. Opening a project
does not contact Spiral, including when no Spiral project has been chosen.
Local feature reads use SQLite. Data-plane requests need a chosen project and
report connection failures at use time. Settings → SpiralDB → Test connection
loads Spiral's runtime and reads the project's catalog as the signed-in user;
Studio control tables are never created in Spiral. See the SQLite guide for persistence and project transfer.

Robot configuration, hardware, and recording live on the robot connector,
which reconnects its saved robots when it starts; see
[robot connector API](robot-connector-api.md). A restarted robot connector
starts idle and motion is never resumed.

The shell owns only the selected project, backend liveness, the robot
connector's address, its clock, and notifications. The robot connector's
`/health` is polled by the active Rollout view, and
its `/robots`, `/hardware/status`, and hardware telemetry only while `/health`
answers, so an absent robot connector costs one request every two seconds and
a browser that never opens Rollout never reaches for it. Each view retains its own last
successful configuration, loading state, and errors until the project scope is
disposed.
Reactivating a hardware view requires fresh setup and telemetry before controls
are enabled.
The shell never fetches robot setup, and non-hardware views do not display cached
robot status. Database errors propagate as feature errors instead of empty robot
lists. Settings preferences are fetched only by the Settings view, whose Save
waits until they have loaded. Its connection settings remain readable without
database access.

Runtime preferences are read from SQLite whenever a feature needs them; the
backend keeps no copy. A training run records the runtime endpoint it starts on
and is followed there for its whole life, so a Settings change applies to the
next run and readiness probe.

SQLite records only what Studio asked each training run to do: its ID,
configuration, start time, and worker endpoint. Each run list asks the run's
worker for its status; opening a run asks for its metrics and log tail, which
the worker owns. A run whose worker no longer holds it, or cannot be reached,
reads as unavailable. Checkpoints are listed from Spiral.
