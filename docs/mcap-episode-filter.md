# Episode inspection: exact recording previews

## September 9 resolution

Spiral PR #7033 (`ed42714c9`) adds UUID-specific comparison overloads. They
compare all 16 bytes, including the version bits, without requiring identical
UUID version metadata. Vortex is unchanged.

The release ADBC driver was rebuilt from `/private/tmp/spiral-uuid-pr` and
installed at `target/release/libadbc_driver_spiraldb.dylib`. The read-only check
below passed against `dataset_default`: two distinct recording IDs returned
549 and 631 joint messages respectively, every returned ID matched its bound
parameter, and the read transaction was rolled back.

Studio now provides **Browse episode topics** from recording details. The
backend binds the selected ID as an Arrow UUID parameter and previews up to 100
messages ordered by log time and sequence. The preview identifies the selected
episode, shows the SQL and bound ID, and offers an explicit **Show all episodes**
action. The topic picker lists dataset topics; a topic absent from the selected
recording returns an empty preview. Video playback remains separate work.

Validation also passed through Studio’s HTTP endpoint for both recordings:
100 correctly scoped messages, ordered by log time and sequence, with truncation
reported. All 444 common/storage/backend tests and the browser-target compile
passed. Workspace Clippy passed with only the existing module-inception and
items-after-test-module findings exempted.

Browser checks confirmed switching between two recordings and displaying the
matching episode metadata. The final **Show all episodes** check cleared the
scope, but the existing unfiltered `/studio/episode` query remained loading;
the same query through HTTP timed out after 60 seconds. Both filtered HTTP
checks still passed during this check. The dataset-wide query issue remains
unresolved; no fallback was added.

The original investigations below describe the previous driver.

## September 8 recheck

The installed local driver still rejects a returned episode ID when it is bound
back into a topic filter. This was reproduced against the current
`dataset_default` resource after the root-catalog migration. No viewer filtering
or playback change has been made around this failure.

The repeatable, read-only check is
[`scripts/check-mcap-episode-filter.py`](../scripts/check-mcap-episode-filter.py):

```sh
uv run --offline --python 3.14 --with pyarrow --with adbc-driver-manager \
  python scripts/check-mcap-episode-filter.py
```

It preserves each returned Arrow ID field and value when binding `$1`, checks
two distinct episodes, and requires every returned message to belong to the
requested member. It rolls back its read transaction. The current failure is
at execution of the first bound topic query, before membership can be checked:
`spql.eq` has no common type for `vortex.uuid[v7]` and `vortex.uuid`.

The upstream UUID hash-join change (`d2564d10d0`) addresses fixed-size-list hash
keys while preserving nominal extension equality; it is not a fix for this
parameter round trip.

## Original investigation

The episode list and metadata selection use `spql.mcap.episodes`, ordered by
recording start time and source URI, with 50 displayed rows per page. Original
filenames are not exposed by this function; Studio numbers episodes in chronological
order and displays the actual recording ID/source in details.

Filtering a topic by the exact episode ID is blocked in the tested driver
built from Spiral commit `2f0ffed61`.

Verified through the driver:

1. Read one episode's `id` as Arrow, without converting it to a string.
2. Prepare this SQL:

   ```sql
   SELECT id, value.episode_id AS original_episode
   FROM spql.mcap.read_topic('default', '/studio/joints/leader')
   WHERE id = $1
   LIMIT 2
   ```

3. Bind a single-row Arrow batch using the returned `id` field and value.
4. Execution rejects equality between `vortex.uuid[v7]` and `vortex.uuid`.

The Arrow UUID importer restores a generic UUID; the stored recording ID is
UUIDv7. Spiral's equality binder rejects these as incompatible. The usual SQL
`CAST('...' AS UUID)` is also unsupported, and casting the ID to VARCHAR fails.
No substitute filename, time, or raw-storage filter was added to Studio.

Next: fix UUID compatibility in Spiral, verify the returned-ID parameter round
trip and exact membership across two different episodes, then wire the filtered
query into the selected episode's topic previews. Approval was requested for the
new driver fix. Dataset-wide topic previews remain explicitly labelled.
