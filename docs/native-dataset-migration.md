# Native recording datasets in Spiral

Status: superseded by the user-approved MCAP migration on September 7, 2026.
The 52 existing recordings are backed up as self-contained MCAPs and imported
as the native Spiral Dataset `default`. All 66,549 decoded messages and all 52
source-file hashes match the backup. The five legacy recording tables and their
three derived video resources were removed after explicit approval; the new
Dataset and all unrelated resources were verified after reconnect. See [the current import design](mcap-import-design.md)
and `exports/native-mcap-2026-09-07/spiral-validation.json`.

The writable native-table proposal below is historical; it is not the current
implementation contract. Live MCAP production and browser upload remain future work.

## Intended result

Each native Studio dataset becomes a real Spiral Dataset resource. Its owned
relations are `episodes` (the proposed default output), `episode_streams`,
`joint_samples`, and `video_assets`. Episode and stream identities remain stable.
Project settings, robots, and run management remain project resources.

The inspector discovers actual Dataset resources and their declared outputs
through Spiral. It must not infer ownership from dotted names or treat rows in
Studio's current `datasets` table as native Dataset resources.

## Verified dependency

Studio now uses the standalone ADBC driver; the previous embedded Spiral Rust
dependency and its cached source are not the runtime contract. The configured
local driver is `target/release/libadbc_driver_spiraldb.dylib`, selected by
`nix develop .#local-driver`. Its build dependency file points into
`/private/tmp/rerobot-current-spiral`.

An isolated in-memory probe of this actual library produced:

| Operation | Result |
| --- | --- |
| `CREATE DATASET studio_native_probe` | Rejected: `CREATE DATASET without using`. |
| Create a source view containing an episode ID | Succeeded. |
| Create using `spql.mcap.dataset` | Producer resolves, then fails because the in-memory session has no default relation catalog. This does not prove hosted MCAP creation. |
| Create using `spql.mp4.dataset` | Rejected: unknown Dataset function. |
| Create using `spql.tar.read` | Rejected: unknown Dataset function. |

The matching source requires `USING` and `FROM` and registers only the MCAP
Dataset producer outside tests. It exposes a Dataset probe/adapter model, not
the older multi-output writer contract discussed earlier. No supported
Studio-native producer or SQL operation for adopting arbitrary writable
episode relations was found. ADBC alone cannot install a Rust Dataset producer
into an already-built driver.

This requires a supported Spiral capability and a rebuilt driver before the
Studio migration can be implemented. Do not synthesize an MCAP source, use a
dummy producer, or fabricate Dataset metadata to get around it. The repository
instruction is: "Do not workaround Spiral's bugs/limitations/missing features.
Ask user first."

There is also a related existing dependency: Studio's video indexing currently
uses `CREATE DATASET ... USING spql.mp4.dataset`, which this driver rejects.
The supported replacement must be established before claiming video preservation
or a full export/import round trip.

## Required Spiral contract

Establish a native recording adapter or supported writable Dataset resource that
can:

1. Create a real Dataset with explicit relation schemas and discoverable outputs.
2. Publish additional episodes and their streams, samples, and media together
   through the same project transaction, without rebuilding earlier recordings.
3. Retry an episode idempotently and delete an episode transactionally.
4. Preserve its identity and output ownership across connections and restarts.
5. Reconstruct a dataset from exported native Arrow rows and media using public
   APIs, with no dependence on temporary source files or private payload codecs.

## Migration and transfer plan after the dependency is resolved

1. Prove the contract against hosted Spiral with two episodes, a repeated write,
   rollback, deletion, reconnect, and media inspection before touching user data.
2. Inventory the selected project's Studio catalog and episode relationships;
   distinguish native recordings from imported dataset catalog entries. Record
   a verified export before changing the existing layout.
3. Introduce dataset-owned relation resolution throughout recording, playback,
   queries, training, and Workflow. Preserve identifiers and explicitly handle
   stored qualified-relation references and derived feature caches.
4. Migrate legacy native rows by dataset membership in a project transaction.
   Validate episode/stream/sample/media counts and relationships, and compare
   media bytes or content hashes. A failed migration leaves the old layout usable;
   rerunning it must not duplicate data.
5. Extend the project bundle manifest to describe native datasets and their
   outputs. Export/import must preserve empty datasets, original names and IDs,
   timestamps, schema types, Blob contents, and source references as appropriate.
   Support importing an existing legacy bundle through the same migration logic.
6. Restore the new bundle into an isolated target project and compare it with
   the source; verify playback and another export. Exercise rollback on invalid
   input and conflicting targets before migrating the user's project.

Existing project transfer is in
[`bundle.rs`](../crates/backend/storage/src/transfer/bundle.rs), and currently
serializes a fixed set of shared Studio relations. Moving writes alone would
omit dataset-owned tables from that format; transfer must change in the same
delivery as storage.

## Validation so far

`nix develop .#local-driver --command cargo test -p spiral-robotics-storage --lib
transfer::bundle::tests --locked` passes 27 tests. Four hosted/profile tests
remain ignored, including the real export/import and video round trips. This
establishes the current bundle validation baseline; it does not validate the
proposed migration or resolve the driver's video producer incompatibility.
