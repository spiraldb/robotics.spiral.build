# MCAP import and dataset inspection

Studio stores recordings as native Spiral Dataset members. The current layout
is described in [catalog layout](dataset-namespaces.md); live capture and
portable project files are described in [recording and playback](recording-mcap.md).

## Storage contract

`dataset_source` owns `dataset_name`, `recording`, and `source` (Blob) values.
Each `dataset_source_<name>` view selects that dataset's input rows. The native
`dataset_<name>` resource uses the view with `spql.mcap.dataset`, a `recording`
member key, and a `source` Blob. Publication refreshes and validates the native
population in the same transaction as source ingestion.

Live recording may append to a validated matching dataset. Reimporting the same
recording key and bytes is idempotent; a different payload at that key is refused.
Hugging Face import refuses existing target resources or source rows. Browser
uploads carry bytes, never filesystem paths for the server to open.

The committed Spiral project journal supplies resource kinds, identities, and
versions. Dataset definitions supply topic/schema metadata. ADBC table listings
are not the authority for native Dataset membership. Studio does not maintain
a parallel recording registry or infer ownership from dotted names.

## Inspection

The Data resource tree selects a native Dataset, its episodes, and its topics.
`spql.mcap.episodes('dataset_default')` reads the population;
`spql.mcap.read_topic('dataset_default', '/studio/joints/leader')` reads a topic.
Previews are bounded and derive their columns from the returned Arrow schema.

Episode-scoped reads bind the exact native member UUID as an Arrow parameter.
That identity differs from the original Studio episode ID in message payloads.
Topic inspection exposes the SQL and bound recording ID; **Show all episodes**
explicitly clears the scope. An absent topic returns an empty preview.
Camera decoding and playback follow [the playback contract](episode-playback.md).

## Driver requirement

The pinned published ADBC driver predates the native MCAP query functions.
Use `nix develop .#local-driver` with a current upstream driver; see
[database setup](database-boundary.md). The driver must support UUID parameter
comparisons across the native member metadata. Studio does not substitute
filename/time filters or emulate missing Spiral functions.

Historical import scripts and migration audit receipts belong with their
backups. They are not supported setup instructions for the current application.
