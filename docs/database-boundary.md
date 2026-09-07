# Async database boundary

Storage features depend on `Database`, `DatabaseSession`,
`DatabaseConnection`, and `DatabaseTransaction`. The current implementation is
native Spiral; replacing it with the ADBC shared-library adapter should leave
feature code on these interfaces.

`ProjectStore` accepts an `Arc<dyn Database>`. Application startup constructs the
configured adapter. `SpiralDatabase::new` only captures configuration; the first
async `connect` initializes the session off the async executor. Connections from
one adapter share that session but retain independent connection state.

## Operations

- `query(SqlStatement)` returns an owned `RecordBatchStream` with its schema,
  including when there are no rows.
- `execute_update(SqlStatement)` returns `ExecutionSummary`.
- `ingest(BatchIngestion)` accepts an Arrow stream and an explicit ingestion mode.
- `table_schema` distinguishes missing relations from driver failures.
- `relations` enumerates the durable relations advertised by the selected catalog.
- `begin`, `commit`, and `rollback` consume their handles. Operations inside a
  transaction, including ingestion and schema inspection, use that transaction.

`SqlStatement` implements `From<T>` for `T: Into<String>`, so callers use
`connection.query(sql.into())`. Driver types, blocking workers, error-message
normalization, and shared-library loading belong inside the adapter. Errors cross
the interface as `DatabaseError` with a `DatabaseErrorKind`.

## Result ownership and cleanup

A returned stream owns the resources necessary to outlive its connection handle.
EOF, an error, or stream drop releases those resources. Errors terminate the
stream. A connection rejects another operation with `InvalidState` while a result
is active, rather than queuing behind a stream the caller may not be consuming.
`ProjectConnection` also retains the project operation lease through the stream.

The contract requires nonblocking cleanup of abandoned transactions, after query
execution stops. Dropping a future stops waiting but does not guarantee that an
in-flight write was cancelled or that a completed commit was rolled back.

### Upstream Spiral cancellation blocker

The native adapter currently cannot guarantee rollback after an early result
drop. Dropping Spiral's `MorselStream` invokes `CancelOnDrop`, which requests
cancellation. Engine tasks may still hold the transaction's query lease when the
adapter attempts rollback; Spiral then rejects it with `ActiveQueries`. The query
subsequently stops, but that failed rollback is not retried.

Keep the stronger contract. Do not add polling, delays, or retry workarounds in
Studio. Fix cancellation/completion coordination upstream, update the pinned
Spiral dependency, wire any new completion API into `ConnectionCleanup`, and
re-enable the ignored `dropped_transaction_waits_for_result_before_rollback` test.
An ADBC implementation must satisfy the same lifecycle requirement.

## Verification

Run the regular storage suite:

```sh
cargo test -p spiral-robotics-storage --lib --locked
```

Reproduce the upstream blocker offline:

```sh
cargo test -p spiral-robotics-storage dropped_transaction_waits_for_result_before_rollback -- --ignored
```

`db/contract_tests.rs` contains driver-independent query and persistence checks.
The native adapter runs the query suite offline. To exercise ingestion modes,
transaction visibility, commit, and rollback against a local Spiral API and SpFS,
set `SPIRAL_SCHEMA_TEST_CONFIG` to the existing local client JSON configuration and
run `native_persistence_contract` with `--ignored`. It uses unique relation names
and cleans them up, including when assertions fail.
