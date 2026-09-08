# MCAP discovery: verified behavior

Checked directly against the imported `default` Dataset using the current local
Spiral ADBC driver. Earlier claims that dataset listing and decoded schema access
were missing from the driver were incorrect.

| Operation | Observed result |
| --- | --- |
| `SELECT * FROM information_schema.resources WHERE kind = 'dataset'` | Returns `default` and the preserved Hugging Face video Dataset, correctly marked `dataset`, with resource/version IDs. |
| ADBC `get_objects` at table depth | Incorrectly reports `default` as `TABLE`. |
| `DESCRIBE "default"` through direct ADBC execution | Fails with unsupported resource service `dataset`. |
| `SELECT * FROM spql.mcap.read_topic('default', '/studio/joints/leader') LIMIT 0` | Returns zero rows and the full decoded Arrow schema, including nested position/action fields. |
| `SELECT * FROM spql.meta.source_functions()` | Registered MCAP sources are `spql.mcap.episodes` and `spql.mcap.read_topic`. |

Table-function and scalar-function lists were also checked. No SQL function to
list a Dataset's topic names was found in the running driver. The matching MCAP
plugin registers the two source functions above and the scalar payload reader
`spql.mcap.message_data`. The Dataset HTTP API already supplies topic names,
variants, schemas, and coverage; that information is not absent from Spiral.

The supported SQL resource listing means Studio does not need a separate HTTP
catalog call merely to discover datasets or distinguish tables from datasets.
The current Studio implementation still uses the HTTP catalog path; no discovery
implementation was changed during this verification.

The remaining SQL question is topic-name/variant discovery. The ADBC
classification and `DESCRIBE` failures were fixed by the rebuild below. Decoded
schemas can already be read once a topic is known. The shipped 0.16.26 driver
predates the native MCAP reader functions.

## After rebuilding the local fix

Commit `2f0ffed61` was built unchanged into Studio's local release driver.
The ADBC table-listing bug is fixed: Dataset roots are absent, and existing
child tables remain. `DESCRIBE "default"` now returns its kind, resource ID,
service, and version ID. It does not return topic names. Schema-only MCAP reads
still succeed. Studio's project startup reports ready with the rebuilt library.
The failed results above describe the preceding driver, not this new build.

Studio UI verification also passed at `http://127.0.0.1:62701/data`: selecting
`default` shows 52 episodes and 10 topics, episode preview returns 52 rows, and
selecting `/studio/joints/leader` returns 100 rows with the decoded joint schema
and values. Video playback and a browser MCAP upload flow remain Studio work;
the completed import used the verified import script.
