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

`drivers/spiral-adbc.json` pins the official archive URLs and SHA-256 digests.
The Nix development and CI shells install that archive and set the library
path; `nix build .#spiral-adbc` produces a separate driver package. For a
standalone deployment, include its library beside the server or supply the
absolute override. An upstream validation build can use that same override
without adding a Spiral source dependency to Studio.

**Use a current-upstream build until a qualifying release is published.** The
adapter passed the full hosted persistence contract against a locally built
`c4b58dd53548ff384226b058b90e83555bd623ee` driver on September 7, 2026:
replacement ingestion, schema visibility, relation enumeration, transaction
isolation, commit, rollback, and no-op DELETE all passed. The same build passed
the offline storage suite.

For this workspace's validated local build, run from the repository root:

```sh
export SPIRALDB_ADBC_DRIVER="$(pwd)/target/debug/libadbc_driver_spiraldb.dylib"
```

That debug library was built from an isolated upstream source checkout; it is
not a distributable release and depends on this machine's Homebrew OpenSSL.
Studio's Cargo dependency graph contains no Spiral or Vortex source packages.

As checked on September 7, 2026, the public [release manifest](https://downloads.spiraldb.com/adbc/latest.json)
still advertises 0.16.26, uploaded August 18 UTC. Its hosted replacement and
transactional ingestion fail our persistence contract. The September 4 native
CI artifact also predates the September 7 transaction-scope fix in
[#6993](https://github.com/spiraldb/spiraldb/pull/6993). Do not treat the pinned
published archive as a qualified replacement for the native adapter until the
hosted contract passes. Update the archive pin when a qualifying release is
published; a source fix alone does not update the downloaded library.

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

Run the regular storage and backend suites with the driver path configured:

```sh
cargo test -p spiral-robotics-storage -p spiral-robotics-backend --lib --locked
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
