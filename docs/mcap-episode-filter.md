# Episode inspection: driver issue

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
