# Spiral data-plane database boundary

Studio control-plane records use [local SQLite with SeaORM](studio-sqlite.md).
The ADBC boundary described below serves datasets, queries, ingestion, and model
artifacts.

Storage features depend on `Database`, `DatabaseSession`,
`DatabaseConnection`, and `DatabaseTransaction`. `ProjectStore` receives an
`Arc<dyn Database>`; startup selects `AdbcDatabase`.

The adapter loads Spiral's standalone shared library through `adbc_core` and
`adbc_driver_manager` 0.24, using `AdbcDriverSpiraldbInit` and ADBC 1.1, following
[Spiral's tutorial](https://github.com/spiraldb/spiraldb/blob/develop/docs-next/content/guides/connect/adbc.mdx).
Studio compiles and links no Spiral or Vortex Rust crate.
SQL dialect, project configuration, and stored Arrow schemas still belong to
Spiral; this boundary isolates its client implementation.

## Loading and distribution

Set `SPIRALDB_ADBC_DRIVER` to an absolute path to the standalone library.
Without that override, the adapter loads `libadbc_driver_spiraldb.dylib` on
macOS or `libadbc_driver_spiraldb.so` on Linux alongside the server executable.
Loading and database initialization happen on the first async `connect`.
Endpoints and the signed-in user's access token are passed as database options,
without changing process environment variables. The driver never refreshes the
token: Studio replaces `spiral.access_token` on every open database whenever it
refreshes the session, which requires a driver with that option.
Studio's provider keys, which the backend reads from its environment at
startup, are passed as write-only options: the OpenRouter key as
`spiral.openrouter_api_key`, which the `spql.ai` model functions read, and the
Hugging Face token as `spiral.huggingface_token`. The driver takes them when a
database is created.

`flake.nix` pins the official version and SHA-256 archive digests in
`adbcVersion` and `adbcArchiveHashes`.
The Nix development and CI shells install that archive and set the library
path; `nix build .#spiral-adbc` produces a separate driver package. It keeps the
archive's video runtime beside the driver and points it at the FFmpeg 9
libraries from nixpkgs, which the archive leaves out. For a
standalone deployment, include its library beside the server or supply the
absolute override. The `local-driver` development shell selects an upstream
validation build without adding a Spiral source dependency to Studio.

**Use a current-upstream build until a qualifying release is published.** The
pinned 0.16.26 archive fails the hosted persistence contract (see
[Upstream regressions](#upstream-regressions)) and predates the native MCAP
query functions Data reads episodes with. Update the pin when a release passes
the hosted contract; a source fix alone does not update the downloaded library.

For this workspace's validated local build, run from the repository root:

```sh
nix develop .#local-driver
```

The shell sets `SPIRALDB_ADBC_DRIVER` to the platform's shared library in
`target/release` and checks that it exists. Build it from the sibling SpiralDB
checkout the way upstream CI does: rustup follows upstream's
`rust-toolchain.toml`, and Homebrew supplies the native dependencies
(`brew install protobuf openssl@3 pkgconf ffmpeg flatbuffers`). Run it outside
Studio's Nix shells, whose compiler would override upstream's pin:

```sh
# Run from the upstream Spiral checkout so its toolchain and Cargo configuration apply.
cargo build --release --locked -p adbc-driver-spiraldb \
  --target-dir /absolute/path/to/spiral-robotics/target
```

This is a locally built release library linked against Homebrew's FFmpeg and
OpenSSL, so it is not a standalone redistributable archive. The build also
produces the matching `libspiral_video_runtime` artifact, which the driver
finds on its own; do not place another copy beside the driver.
`local-driver` loads whatever file sits at that path, so it must be this
`--release` build. Never copy a debug build (`cargo build` without `--release`)
into `target/release`: the unoptimized engine made video frame planning and
model preprocessing, not inference, dominate progress scoring.
Rebuilding the driver does not replace a running server's loaded library;
restart Studio after rebuilding.

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

Operations use SQL and Arrow through `query`, `execute_update`, `ingest`, and
`table_schema`. Driver statuses become `DatabaseErrorKind`; missing tables
return `None` from schema inspection. `SqlStatement` implements `From<T>` for
`T: Into<String>`.

## Upstream regressions

`adbc_persistence_contract` checks ingestion, schema visibility, commit, and
rollback. On 0.16.26 it fails with
`hosted Arbor CREATE OR REPLACE TABLE is not supported`.
`adbc_transactional_ingestion_stays_isolated` independently reproduces writes
becoming visible to another connection before commit in that released library.
Current upstream fixes both. Keep these regression tests, and rerun them when
updating the published archive pin.

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

The local test configuration contains `api_url`, `spfs_url`, `project_id`, and
`access_token`, a user token such as `spiraldb login --show-token` prints; it
lasts about five minutes. The hosted tests require loopback
endpoints, create unique table names, and remove their tables afterward:

```sh
export SPIRAL_SCHEMA_TEST_CONFIG=/absolute/path/to/local-client.json
cargo test -p spiral-robotics-storage adbc_persistence_contract -- --ignored
cargo test -p spiral-robotics-storage adbc_transactional_ingestion_stays_isolated -- --ignored
cargo test -p spiral-robotics-storage adbc_hosted_noop_delete_succeeds -- --ignored
cargo test -p spiral-robotics-storage dropped_transaction_waits_for_result_before_rollback -- --ignored
```

## Startup and schema checks

SQLite opens and migrates before the HTTP listener starts. Opening a project
does not contact Spiral, including when no Spiral project has been chosen.
Local feature reads use SQLite. Data-plane requests need a chosen project and
report connection failures at use time. Saving another project and Settings →
SpiralDB → Test connection validate and provision the model-artifact relation
only; Studio control tables are never created in Spiral. See the SQLite guide for persistence and project transfer.

Robot configuration, hardware, and rollout live on the robot connector, which
reconnects its saved robots when it starts; see
[robot connector API](robot-connector-api.md). Its rollout keeps only its
current session, in memory; a restarted robot connector starts idle and motion
is never resumed. The Rollout view owns loading/error feedback and retains its
last successful snapshot if polling fails. Its execution remains owned by the
robot connector after navigating away.

The shell owns only the selected project, backend liveness, the robot
connector's address, its clock, and notifications. The robot connector's
`/health` is polled by the active Live or Rollout view or Settings → Robots, and
its `/robots`, `/hardware/status`, and hardware telemetry only while `/health`
answers, so an absent robot connector costs one request every two seconds and
a browser that opens none of these views never reaches for it. Each view retains its own last
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
