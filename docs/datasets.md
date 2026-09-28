# Datasets and recordings

A dataset is a native Spiral Dataset backed by MCAP files. Live recording and
Hugging Face import both produce those files; they are not different dataset
kinds, and readers never ask which one produced an episode.

```text
Live capture → complete MCAP bytes ─┐
                                   ├→ dataset_source → dataset_<name> → Data, Curation
Hugging Face → pinned Blob URIs ───┘
```

## Catalog layout

The selected Spiral project holds Studio's resources at its root:

| Resource | Kind | Purpose |
| --- | --- | --- |
| `dataset_source` | Table | Every managed MCAP source: `dataset_name`, `recording` (an opaque source key within its dataset, not an episode identity), and the `source` Blob |
| `dataset_source_<name>` | View | Selects `recording, source` where `dataset_name = '<name>'` |
| `dataset_<name>` | Native Dataset | Reads its source view through `spql.mcap.dataset`, with `recording` as the member key |
| `studio_training_checkpoints` | Table | Model checkpoint artifacts |

Curation publishes its views at the root too. A Dataset refresh consumes its
view's rows and updates the Dataset's native population, so datasets share one
source table without sharing recordings. The definition's `sourceRelation`
records the association; ownership is never inferred from names. Source views
sit at the root because Spiral's child-view path needs keyed parent metadata
that Arbor does not expose. The source table declares no primary key.

Publication creates or appends the source table, creates the view, creates and
refreshes the Dataset, and verifies its member count in one transaction.
Publishing the same recording key with the same bytes again is an idempotent
retry; different bytes at that key are refused. Hugging Face import refuses
existing target resources or source rows with the same dataset name; Live
recording appends to a validated matching Dataset. Browser uploads carry
bytes, never filesystem paths for the server to open. The committed Spiral
project journal supplies resource kinds, identities, and versions; ADBC table
listings are not the authority for Dataset membership. Studio does not write
to an arbitrary external dataset.

## Recording

A completed Live recording becomes one immutable `.mcap` file: the recording
header, stream metadata, joint observations and actions, and H.264 camera
access units. Camera messages use `foxglove.CompressedVideo`; joint and
metadata messages use the `studio.recording` Protobuf schemas. On final stop,
Studio saves and syncs the file under the platform local-data folder, at
`spiral-robotics/recordings/<project-hash>/<dataset>/<episode-id>.mcap`, then
publishes it as above. If publication fails, the error names the saved file;
nothing retries it yet. A stop with `keep: false` discards the take instead.

Pausing ends the current encoder segment without publishing an incomplete
member; resuming keeps the episode's elapsed clock, excluding the pause. Frame
timestamps use capture time, not frame number. Every keyframe carries its
decoder parameters, so seeking starts from the preceding keyframe. Calibration
changes affect future recordings only: a recording keeps the normalized values
captured with it, and angles in degrees need a calibration snapshot.

Live reviews the current take from its session, never from files.
`RecordingBuffer` holds only the current session's capture and review, and
camera preview keeps that session's encoded packets, at most 18,000 packets and
64 MiB per camera, until a new capture starts. `RecordingSession` holds the one
open take and the last one kept for review: an operation that changes the take
(start, pause, stop, or closing a take the control loop ended) holds it
throughout, operations never interleave, and a cancelled one leaves the take
as it was. These routes live under `/recording/takes/{id}`, whose IDs name
session captures, not dataset members. Once the session is replaced or Studio
restarts, the take is read through Data like any other episode.

Automated tests never move hardware; physical capture still needs a smoke
test with a real robot and camera.

## Reading episodes

Readers use dataset identity and version, member UUID, topic and schema
revision, timestamps, and values, and learn what a stream holds from its
schema. They never read the source table, reconstruct filenames, or require
Studio's own topics or capture types; the metadata Studio writes into its
files is ordinary optional content. `spql.mcap.episodes('dataset_default')`
reads a population and `spql.mcap.read_topic('dataset_default',
'/studio/joints/leader')` reads a topic. [Episode playback](episode-playback.md)
describes the inspection built on them.

Episode-scoped reads bind the member UUID as `$1`. The driver must compare the
UUIDv7 in member metadata with the generic UUID that Arrow parameter binding
restores (Spiral PR #7033, `ed42714c9`); no filename, timestamp, text-cast, or
raw-storage filter substitutes for member identity. The pinned published
driver predates the MCAP query functions, so use `nix develop .#local-driver`
(see [database setup](database-boundary.md)).

Project bundles carry the original MCAP bytes and Dataset names, including
empty datasets; import validates ownership and rebuilds the views and
populations in the same transaction as Studio state. See
[project transfer](studio-sqlite.md#project-transfer).

## Training and rollout

Training is parked until Spiral decodes MCAP video into training images:
Studio refuses to start a run, and there is no Studio-side decoder. Rollout
runs existing checkpoints whose schema matches exactly.
