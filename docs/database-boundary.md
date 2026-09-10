# Async database boundary

Storage features depend on `Database`, `DatabaseSession`,
`DatabaseConnection`, and `DatabaseTransaction`. `ProjectStore` receives an
`Arc<dyn Database>`; startup selects `AdbcDatabase`. The trait and feature
callers remain unchanged by the driver migration.

The adapter loads Spiral's standalone shared library through `adbc_core` and
`adbc_driver_manager` 0.24, using `AdbcDriverSpiraldbInit` and ADBC 1.1, following
[Spiral's tutorial](https://github.com/spiraldb/spiraldb/blob/develop/docs-next/content/guides/connect/adbc.mdx).
Studio no longer compiles or links the native Spiral or Vortex Rust crates.
SQL dialect, project configuration, and stored Arrow schemas still belong to
Spiral; this boundary isolates its client implementation.

## Loading and distribution

Set `SPIRALDB_ADBC_DRIVER` to an absolute path to the standalone library.
Without that override, the adapter loads `libadbc_driver_spiraldb.dylib` on
macOS or `libadbc_driver_spiraldb.so` on Linux alongside the server executable.
Loading and database initialization happen on the first async `connect`.
Endpoints and credentials are passed as database options, without changing
process environment variables.

`flake.nix` pins the official version and SHA-256 archive digests in
`adbcVersion` and `adbcArchiveHashes`.
The Nix development and CI shells install that archive and set the library
path; `nix build .#spiral-adbc` produces a separate driver package. For a
standalone deployment, include its library beside the server or supply the
absolute override. The `local-driver` development shell selects an upstream
validation build without adding a Spiral source dependency to Studio.

**Use a current-upstream build until a qualifying release is published.** The
adapter passed the full hosted persistence contract against a locally built
`c4b58dd53548ff384226b058b90e83555bd623ee` driver on September 7, 2026:
replacement ingestion, schema visibility, relation enumeration, transaction
isolation, commit, rollback, and no-op DELETE all passed. The same build passed
the offline storage suite.

For this workspace's validated local build, run from the repository root:

```sh
nix develop .#local-driver
```

The shell sets `SPIRALDB_ADBC_DRIVER` to the platform's shared library in
`target/release` and checks that it exists. Build it from an isolated upstream
source checkout using the flake's `adbc-build` shell, which supplies upstream's
August 10 compiler, OpenSSL, and FFmpeg while Studio keeps its own compiler pin:

```sh
# Run from the upstream Spiral checkout so its Cargo configuration applies.
nix develop /absolute/path/to/rerobot#adbc-build --command cargo build \
  --release --locked -p adbc-driver-spiraldb \
  --target-dir /absolute/path/to/rerobot/target
```

This is a locally built release library; its native dependencies live in the
Nix store, so it is not a standalone redistributable archive.
The September 7 release build is 185.2 MiB and passes all 375 regular
storage/backend tests plus the three hosted persistence regressions.
Studio's Cargo dependency graph contains no Spiral or Vortex source packages.

As checked on September 7, 2026, the public [release manifest](https://downloads.spiraldb.com/adbc/latest.json)
still advertises 0.16.26, uploaded August 18 UTC. Its hosted replacement and
transactional ingestion fail our persistence contract. The September 4 native
CI artifact also predates the September 7 transaction-scope fix in
[#6993](https://github.com/spiraldb/spiraldb/pull/6993). Do not treat the pinned
published archive as a qualified replacement for the native adapter until the
hosted contract passes. Update the archive pin when a qualifying release is
published; a source fix alone does not update the downloaded library.

## Current local driver refresh

The schema inspector uses local `xiangpeng/dev` commit
`02d06b5c59df0d67bc2ca9c4624520e24e328af5`, rebuilt on September 9, 2026 with
the `adbc-build` command above. It includes `320d50ddb`, which fixes
demand-pruning convergence for nested projections. The local library SHA-256 is
`be631980da06733ddc2e26624a81f76dc18d84dcab65b7b5272008329ee85751`.
Studio's verification preview on port 8091 was restarted with that library;
recorded position/action arrays now load through the normal projected Arrow
transport. Other running Studio processes must restart to load the new library.
This does not update the public archive pinned by the default Nix shell.

### Earlier catalog validation

The local driver was rebuilt from the unchanged checkout
`/private/tmp/spiraldb-dataset-catalog-pr`, commit `2f0ffed61`, which includes the
Dataset classification fix and current MCAP readers. Studio on port 62701 was
restarted with this library and reports its project ready. Direct checks confirm
that ADBC table listing excludes datasets while retaining child tables,
`DESCRIBE "default"` succeeds, and the imported 52 episodes / 66,549 messages
remain readable. The library hash and results are recorded in
`exports/native-mcap-2026-09-07/driver-rebuild-validation.json`.

## Async execution and ownership

- Blocking initialization, connections, operations, reader polling, and driver
  cleanup run off the async executor. An async mutex serializes each connection;
  idle connections occupy no worker threads.
- A query uses one blocking job for its reader lifetime. A two-batch channel
  provides backpressure without a task handoff per row or per batch. The driver
  retains its own bounded read-ahead. Separate connections execute independently.
- Ingestion exposes the input as a synchronous Arrow reader. Its blocking caller
  polls the async input through the captured Tokio handle, pulling one batch at
  a time without collecting the input or starting an extra pump task.
- Returned streams retain their schema, connection, and project operation lease.
  An active result makes other operations fail with `InvalidState`. EOF, error,
  and stream drop release ownership; subsequent workers await reader release.
- Imported Arrow buffers retain the driver library even after their stream,
  connection, and database have been dropped. Slices and cloned columns preserve
  that ownership. Buffer contents are shared without copying.
- `begin`, `commit`, and `rollback` consume their handles. Transactions map to
  ADBC autocommit control and connection commit/rollback. Abandoned transactions
  schedule rollback after the result worker releases its reader and statement.
  Dropping a future stops waiting; it does not guarantee cancellation of an
  in-flight write or reversal of a completed commit.

Operations use SQL and Arrow through `query`, `execute_update`, `ingest`,
`table_schema`, and `relations`. Driver statuses become `DatabaseErrorKind`;
missing tables return `None` from schema inspection. Metadata enumeration
filters the exact selected project as well as passing ADBC search patterns.
`SqlStatement` implements `From<T>` for `T: Into<String>`.

## Upstream regressions

`adbc_persistence_contract` checks all ingestion modes, schema visibility,
relation enumeration, commit, and rollback. On 0.16.26 it fails with
`hosted Arbor CREATE OR REPLACE TABLE is not supported`.
`adbc_transactional_ingestion_stays_isolated` independently reproduces writes
becoming visible to another connection before commit in that released library.
Current upstream fixes both failures: all hosted tests pass with the locally
built `c4b58dd5` standalone library. Keep these regression tests, and rerun them
when updating the published archive pin.

`dropped_transaction_waits_for_result_before_rollback` preserves the earlier
cancellation regression. Arrow reader release requests cancellation, but the
engine may still hold an active-query lease when rollback starts. Keep the
stronger contract and the documented ignore until upstream guarantees query
completion before cleanup rollback. A passing run of a race-sensitive test
alone does not establish that guarantee. No polling, delays, retries, draining,
or native fallback is added to Studio.

## Verification

Run the regular storage and backend suites through the flake's validation shell:

```sh
nix develop .#local-driver --command cargo test -p spiral-robotics-storage -p spiral-robotics-backend --lib --locked
```

The storage suite includes offline execution through the actual shared library,
resource ownership, exact Arrow allocation retention, and async input polling.

The existing local test configuration contains `api_url`, `spfs_url`,
`project_id`, `client_id`, and `client_secret`. The hosted tests require loopback
endpoints, create unique table names, and remove their tables afterward:

```sh
export SPIRAL_SCHEMA_TEST_CONFIG=/absolute/path/to/local-client.json
cargo test -p spiral-robotics-storage adbc_persistence_contract -- --ignored
cargo test -p spiral-robotics-storage adbc_transactional_ingestion_stays_isolated -- --ignored
cargo test -p spiral-robotics-storage adbc_hosted_noop_delete_succeeds -- --ignored
cargo test -p spiral-robotics-storage dropped_transaction_waits_for_result_before_rollback -- --ignored
```

## Startup and schema checks

Resuming the saved project publishes readiness without database I/O. It does
not inspect the nine managed schemas or load preferences, robots, or rollout
state. First-time project selection, project changes, and Settings → SpiralDB →
Test connection still provision and validate managed relations before publishing
the destination. A CLI/environment project override that differs from the saved
project also takes this validation path. Missing or incompatible feature data on resume is reported by
that feature when accessed.

Robot configuration is read by the project-scoped robot workspace when needed by
the hardware views, robot commands, and saved-device auto-connect. The discarded startup
validation read is gone; auto-connect still runs after project publication.
Rollout history loads once per project session on the first Rollout snapshot or
start command. Concurrent callers share initialization, failed or cancelled reads
leave it available for a later request, and a restored active snapshot becomes
failed without resuming motion. The Rollout view owns loading/error feedback and
retains its last successful snapshot if polling fails. Its execution and
persistence remain owned by the backend after navigating away. Stop does not wait
for history I/O.

The shell owns only project identity/readiness, connection status, its clock, and
notifications. `/robots/setup` and hardware telemetry are polled by the active
Live, Rollout, or Settings view; each view retains its own last successful
configuration, loading state, and errors until the project scope is disposed.
Reactivating a hardware view requires fresh setup and telemetry before controls
are enabled.
The shell never fetches robot setup, and non-hardware views do not display cached
robot status. Database errors propagate as feature errors instead of empty robot
lists. Settings preferences are fetched only by the Settings view. Its connection
settings remain readable without database access; a connection-only update
validates and switches the project without overwriting unread preferences.

Backend runtime preferences are shared within one project lifetime and loaded
once on first Settings, Training, Rollout start, or recording-video access.
Concurrent readers share initialization; errors and cancellation leave it
retryable. A preference write wins over an in-flight first read. Training workers
reject uninitialized preferences instead of choosing a default endpoint.

Training history, Workflow, Assistant history, and the full relation catalog also
load on feature access rather than blocking project readiness.

Managed-table verification and relation discovery read schemas concurrently on
up to eight independent ADBC connections. The reads retain their parent project
operation lock and preserve input order and individual errors. Parallel schema
reads reject explicit transactions, whose schema reads must use that transaction.
