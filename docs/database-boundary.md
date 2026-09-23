# Spiral data-plane database boundary

Studio control-plane records use [local SQLite with SeaORM](studio-sqlite.md).
The ADBC boundary described below serves datasets, queries, ingestion, and model
artifacts. Historical driver validations below describe their original scope.

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
Endpoints and the signed-in user's access token are passed as database options,
without changing process environment variables. The driver never refreshes the
token: Studio replaces `spiral.access_token` on every open database whenever it
refreshes the session, which requires a driver with that option.

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
`target/release` and checks that it exists. Build it directly from the sibling
SpiralDB checkout using the flake's `adbc-build` shell, which supplies upstream's
August 10 compiler, OpenSSL, and FFmpeg while Studio keeps its own compiler pin:

```sh
# Run from the upstream Spiral checkout so its Cargo configuration applies.
nix develop /absolute/path/to/spiral-robotics#adbc-build --command cargo build \
  --release --locked -p adbc-driver-spiraldb \
  --target-dir /absolute/path/to/spiral-robotics/target
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

## Structured plan driver verification

The Dioxus Flow plan display uses a release rebuild from upstream
`41e373f4b5d52df37cb4d422579aab6aea0f065b`, including EXPLAIN JSON schema version 2.
The library SHA-256 is
`1d29c533609e3bbc2e07e30947183d0f17bb9aa3936501f40ace8ecb4419a10b`.
Logical, optimized logical, and physical plan fields now contain versioned
node/edge diagnostics. Text trees remain under the `*_plan_text` keys. Studio
requires this structured contract and no longer uses Graphviz.
Rebuild with `adbc-build`, run with `local-driver`, and restart existing servers.

## SQL workflow driver verification

On September 11, 2026, the SQL workflow checks used a release rebuild from the
local upstream checkout at `aca5089b16bbeaa12ae988abc4e3a4b0c948fb52`, including
its in-progress ADBC EXPLAIN support. Studio did not modify upstream source.
The library SHA-256 is
`070d58d8b1f94db7ea50a118a74e8c6f7d3ce0a2b3e3bcfbea0af86a7324d62c`.
Use `local-driver` after rebuilding with the command above. Existing processes
must restart to load it. The default public archive remains unchanged.

Native SQL execution, AI capability discovery, null-input `spql.ai.score`, and
`EXPLAIN FORMAT GRAPHVIZ` rendering pass Studio's integration tests.
`EXPLAIN (FORMAT JSON)` also works, but its logical and physical plan fields
are formatted strings. Structured node/edge plan export is still separate work.
These checks do not verify non-null model inference on complete episodes.

## Current local driver refresh

September 14, 2026: the decimal-to-integer cast fix is merged upstream in Vortex
commit `0a2d04c7163114b36f6e821c96774b5439d99125` ([PR #9873](https://github.com/vortex-data/vortex/pull/9873)).
SpiralDB commit `a2ac94916` pins that revision, and the current checkout already
uses compatible protobuf APIs. Build the driver directly from that checkout
using the command above; no Studio patch or local Vortex fork is required.
The `adbc-build` shell supplies the required FlatBuffers 25.12.19 compiler.

The earlier local-fork validation installed a debug driver with SHA-256
`60ceb4f136ea949899db6c80b345b4c48106fa211eb94ac1339d82cf39668bf0` at
`target/release/libadbc_driver_spiraldb.dylib`. The directory name does not
indicate its build profile. Rebuilding the driver does not replace a running
server's loaded library; restart Studio after rebuilding.

Validation passed all 377 decimal-related Vortex tests, the SPQL protobuf feature
check, and the original `CAST(row_number() OVER () - 1 AS BIGINT)` reproduction
through Studio. The two-episode decoder evaluation returned 12 images, including
the episode that previously failed with a missing H.264 parameter set. Its SQL
uses `row_number()` ordered by `(chunk_start_offset, sample_ordinal)` within each
episode, because `sample_ordinal` alone resets across chunks. These checks cover
casting and decoding; they do not establish object-holding detection accuracy.

September 12, 2026: Settings → SpiralDB accepts a write-only Hugging Face token.
Studio passes it as `spiral.huggingface_token` when opening the ADBC database.
This requires the local upstream HF-token support merged at
`5eaad38bd4d150efe8525c2a8bdbd8a5266a0152`; the public archive pin is unchanged.

The Settings preview on port 8094 uses that driver's **debug build**, copied to
`target/release/libadbc_driver_spiraldb.dylib` so `local-driver` selects it.
The directory name does not indicate this artifact's build profile. Its SHA-256 is
`8438918bab084281b9e06b15b1c79c6e3a506660200c6a946bd8a400718d24d6`.
The build command was `cargo build -p adbc-driver-spiraldb` in the upstream checkout.
Tests verified private token persistence, preserve/replace/remove updates, and the
native write-only option boundary. Non-null inference remains unverified.

### Previous production Dataset repair

The local driver was rebuilt on September 11, 2026 from upstream
`0435bb5b0fdff2aec21b537fc702333769e49215` (the production Dataset repair).
The SHA-256 of `target/release/libadbc_driver_spiraldb.dylib` is
`82ee25be993d13ab88261651563d83f1b43c7a7ce94692857f6870d3ee438e2b`.
Studio on port 8080 was restarted and confirmed to load this library.

The old driver rejected the production Dataset response's `extent` field.
The new driver passed the live viewer API checks: all 52 episodes across
pagination, ten topics for the first episode, joint and camera schemas,
16-message pages, and the camera's projected format/data fields.
The episode page took 1.10 seconds; sampled message reads took 2.61–3.42 seconds.
These checks cover the backend API and Arrow payloads; browser rendering and
WebCodecs playback were not tested. Details are recorded in
`exports/driver-refresh-2026-09-11/validation.json`.

The source build used an isolated checkout at
`/private/tmp/spiral-driver-prod-20260911` and the existing `adbc-build` shell.
The upstream development checkout was left unchanged. The public archive pin
in the default Nix shell is unchanged; use `nix develop .#local-driver`.

### Previous packet-filter driver refresh

The workflow verification preview on port 8080 uses local `xiangpeng/dev` commit
`fe0726b14c8256626060e3caf48681b169a48390`, rebuilt in release mode on September 10,
2026 with the `adbc-build` command above. The library SHA-256 is
`0cc4d559e0db14d837a41f88def3820b871469d18439abb00ce07926eb3688ab`.
This includes packet video decoding and join-filter publication gates across
streaming exchanges. Live IN and JOIN queries now read only the selected 274 MCAP
camera packets instead of 10,990; decoded images are unchanged. The native
H.264 → SigLIP → SQL workflow regression passes twice without score reuse.
Other running Studio processes must restart to load this library. The default
Nix shell's public archive pin is unchanged.

### Earlier schema inspector validation

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

Robot configuration is read by the project-scoped robot workspace when needed by
the hardware views, robot commands, and saved-device auto-connect. The discarded startup
validation read is gone; auto-connect runs for the saved project at startup and
for a project once a settings save switches to it.
Rollout keeps only its current session, in memory; a restarted Studio starts
idle and motion is never resumed. The Rollout view owns loading/error feedback
and retains its last successful snapshot if polling fails. Its execution
remains owned by the backend after navigating away.

The shell owns only the selected project, backend liveness, its clock, and
notifications. `/robots`, `/hardware/status`, and hardware telemetry are polled by the active
Live, Rollout, or Settings view; each view retains its own last successful
configuration, loading state, and errors until the project scope is disposed.
Reactivating a hardware view requires fresh setup and telemetry before controls
are enabled.
The shell never fetches robot setup, and non-hardware views do not display cached
robot status. Database errors propagate as feature errors instead of empty robot
lists. Settings preferences are fetched only by the Settings view. Its connection
settings remain readable without database access; a connection-only update
validates and switches the project without overwriting unread preferences.

Runtime preferences are read from SQLite whenever a feature needs them; the
backend keeps no copy. A training run records the runtime endpoint it starts on
and is followed there for its whole life, so a Settings change applies to the
next run and readiness probe.

SQLite records only what Studio asked each training run to do: its ID,
configuration, start time, and worker endpoint. Each run list asks the run's
worker for its status; opening a run asks for its metrics and log tail, which
the worker owns. A run whose worker no longer holds it, or cannot be reached,
reads as unavailable. Checkpoints are listed from Spiral.
Workflow, Assistant history, and the full relation catalog also load on feature
access rather than when a project opens.

Managed-table verification and relation discovery read schemas concurrently on
up to eight independent ADBC connections. The reads retain their parent project
operation lock and preserve input order and individual errors. Parallel schema
reads reject explicit transactions, whose schema reads must use that transaction.
