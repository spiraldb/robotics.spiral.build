# MCAP import and dataset inspection

Update, September 8: the dataset and its source now live at
`dataset_default` and the root `dataset_source_default` view of `dataset_source`. The commands below
record the original migration. See [current catalog layout](dataset-namespaces.md).

September 7, 2026. This replaces the older MCAP output-table assumptions in
`data-viewer-study.md`. The installed ADBC driver uses native Dataset resources,
`spql.mcap.episodes`, and `spql.mcap.read_topic`; it does not generate `files`,
`topics`, or `messages` child tables.

## This phase

Import and inspection come first. Existing native recordings have been exported
as 52 MCAP files with H.264 camera topics, original MP4 attachments, exact native
row attachments, timestamps, checksums, and verified standalone restoration.
The import uses those files as the source of one native Dataset named `default`.
Moving future Live recording to MCAP is a separate producer change.

## Storage and publication

1. Accept a dataset name and an explicit selection of local MCAP files. Browser
   files are uploaded bytes, never paths for the backend to open.
2. Store each original filename, byte count, SHA-256, and a Spiral-owned Blob in
   a durable source relation. MCAP bytes go through ADBC ingestion; no filesystem
   spool, fake Dataset, or message-by-message copy into application tables.
3. Create the Dataset definition using the supported command API:

   ```sql
   CREATE DATASET "default" USING spql.mcap.dataset
   FROM studio_mcap_sources_37a8eec1ce19
   WITH (key = 'recording', blob = 'source', on_error = 'fail');
   REFRESH DATASET "default";
   ```

   Creation records the definition; refresh discovers the finite episode
   population and schema catalog. Refresh does not eagerly decode every frame.
4. Validate the episode and message counts, then commit. Refuse existing targets;
   do not silently replace an existing Dataset or append duplicate recordings.
   Reconnect and verify the published population. A commit error is an uncertain
   outcome: inspect the target before retrying.
5. Keep the source relation: it owns the durable input Blobs and supports later
   refresh. It is not a second application-level dataset catalog.

The one-off importer in `exports/native-mcap-2026-09-07/tools/import_to_spiral.py`
implements this transaction with one Arrow batch per file and an import receipt.
It uses direct ADBC command execution for DDL, matching Studio's Rust adapter;
Python DB-API prepared SELECT execution is not the Dataset DDL interface.

## What “typed project journal” means

It is Spiral's versioned resource catalog for one project. “Typed” means each
entry explicitly says whether it is a Dataset, Table, or View; it does not mean
a new schema for the recorded messages. “Journal” is the API's name for the
project's catalog history; Studio reads its current committed snapshot.

For this migration the catalog distinguishes the `default` Dataset from the
`studio_mcap_sources_37a8eec1ce19` Table that owns its input MCAP Blobs. Studio
would use this catalog to discover resources, then continue using ADBC/SQL to
read table rows and MCAP topic messages. It does not move or duplicate data.

## Discovery correction

Direct verification found a supported SQL alternative to the HTTP catalog path:
`SELECT * FROM information_schema.resources WHERE kind = 'dataset'` correctly
lists the imported datasets. A known topic's decoded schema is also available
through `spql.mcap.read_topic(... ) LIMIT 0`. These are not missing driver features.
See [the verified results](mcap-discovery-check.md). No SQL topic-name listing was
found in the tested driver; the Dataset HTTP API already provides those details.
The HTTP-based implementation described below has not yet been replaced.

## Studio interaction

The Data workbench starts with a list of actual Spiral Dataset resources.
Selecting a dataset shows its episode population and available native topics.
Episode filtering is a next inspector step, using Spiral member ID; original Studio IDs
remain in the `/studio/episode` and joint message payloads. Dataset-native UUIDs
and the original `episode_000001` identifiers are distinct identities.

Studio reads `/projects/{project}/journal/branches/main`, then pages
`/projects/{project}/journal/state/read` at that commit to enumerate typed
Dataset resources and namespace ownership. It reads `/datasets/{id}/versions/{version}`
for the selected exact version and declared observation schemas. ADBC table enumeration lists Tables and Views;
it is not authority for Dataset membership. Do not reconstruct ownership from
name prefixes or repopulate `spiral_robotics.datasets` as a parallel catalog.

Use the existing Results panel for bounded structured previews, with a visible
SQL action. Read the population through:

```sql
SELECT * FROM spql.mcap.episodes('default') LIMIT 100;
```

Read one topic through:

```sql
SELECT * FROM spql.mcap.read_topic('default', '/studio/joints/leader') LIMIT 100;
```

Topic rows include member `id`, exact integer `log_time` and `publish_time`,
channel and sequence metadata, and decoded `value` fields. Preview columns come
from the returned schema. A camera topic contains `foxglove.CompressedVideo`;
showing its decoded fields is separate from playing its H.264 stream. Playback
must begin at an IDR and preserve one shared presentation clock. Do not claim
video playback solely because Protobuf decoding succeeds.

Preserve the existing light desktop workbench, mouse/keyboard controls, and 1024
CSS-pixel minimum. Reuse existing actions, tables, loading feedback, and query
results. Import presents filename/count/bytes, a target name, and measured
upload progress. Indexing is a named phase without an invented percentage.
After successful publication, select the actual Dataset and open its episodes.
Errors retain file selection and an actionable phase; retries must check what
was already published.

A production browser uploader should stage one file per bounded HTTP request
and persist its receipt before final publication. An interrupted browser must
not erase the import identity. The one-off importer is an operational migration
tool, not evidence that this browser workflow is implemented.

## Cleanup boundary

Retire only the backed-up native recording rows and their generated video
indexes after the published MCAP data has been checked. Inventory current data
again before deletion so new recordings cannot be discarded against a stale
backup. Preserve unrelated robots, preferences, training/workflow history, and
Hugging Face source relations.

Startup now verifies/provisions only the nine application-state relations.
It does not recreate the five retired recording tables. Data uses the native
Dataset inspector, and Live refuses new recording before hardware capture
when legacy storage is absent. The next producer change is writing MCAP from
Live. Legacy episode playback, training selection, and Hugging Face import UI
have not been adapted to native MCAP members.

Project backup must retain Dataset definitions and exact source MCAP bytes, then
recreate/refresh native datasets on import. Existing fixed-table `SPRPROJ1`
bundles do not automatically include native Dataset resources. The validated
MCAP backup remains the recovery artifact for this migration.

## Driver requirement

The shipped `0.16.26` driver does not register `spql.mcap.episodes`. This phase
uses the already-built current upstream driver at
`target/release/libadbc_driver_spiraldb.dylib`. Start Studio from the
`nix develop .#local-driver` shell, then `cd crates/studio` and `dx serve`.
The default Nix release pin still needs a compatible published driver release;
there is no compatibility shim or emulation of the missing query functions.

## Verification and remaining integration

The remote import is committed and fully verified: 52 episodes, 10 topics,
66,549 decoded messages, and 52 identical source MCAP hashes. The new dataset
list displayed the real `default` resource and all topic/schema counts in Studio.
Native MCAP queries were verified directly through the current ADBC driver.

End-to-end Studio preview works after rebuilding Spiral commit `2f0ffed61`:
ADBC table enumeration excludes Dataset resources, startup completes, and the
inspector displays all 52 episodes and a 100-row decoded joint-topic preview.
The supported SQL `information_schema.resources` query can list datasets;
the inspector currently uses the Dataset HTTP API for listing and topic details.
No SQL topic-name listing was found in the tested driver. See
[the discovery checks](mcap-discovery-check.md). The shipped driver still lacks
the native MCAP query functions, so use the local-driver shell.

Physical cleanup is complete after explicit user approval to remove the old
VideoIndex without a separate index backup. All original rows and MP4 bytes
were rechecked against the backup before deletion. The transaction removed:

- `spiral_robotics.datasets`, `spiral_robotics.episodes`,
  `spiral_robotics.episode_streams`, `spiral_robotics.joint_samples`, and
  `spiral_robotics.video_assets`.
- `spiral_robotics.video.assets` (derived table).
- `spiral_robotics.video` (old Dataset).
- `spiral_robotics_video_sources` (source view).

Reconnection confirmed all eight removals, every unrelated resource's identity,
and the imported Dataset's 52 episodes and 66,549 messages. The audit and receipt
are in `exports/native-mcap-2026-09-07/cleanup-audit-approved/` and
`exports/native-mcap-2026-09-07/spiral-cleanup.json`.

Unrelated Hugging Face, robot, preference, training, rollout, and workflow
resources remain outside the cleanup scope.
