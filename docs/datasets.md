# Datasets and recordings

A dataset is a native Spiral Dataset backed by MCAP files. Rollout recording and
Hugging Face import both produce those files; they are not different dataset
kinds, and readers never ask which one produced an episode.

```text
Rollout capture on the robot connector → browser → complete MCAP bytes ─┐
                                                                    ├→ dataset_files_<name> → dataset_<name> → Data, Curation
Hugging Face → pinned Blob URIs ────────────────────────────────────┘
```

## Catalog layout

The selected Spiral project holds Studio's resources at its root:

| Resource | Kind | Purpose |
| --- | --- | --- |
| `dataset_files_<name>` | Table | The dataset's MCAP files: `recording` (an opaque source key within its dataset, not an episode identity) and the `source` Blob |
| `dataset_<name>` | Native Dataset | Made from its files table through `spql.mcap.dataset`, with `recording` as the member key and `materialization = 'all'` |
| `dataset_answers_<name>` | Table | One row per episode, keyed by `episode_id`, and one column per exact model call a curation made, each value `STRUCT(value …)`: NULL not asked yet, a NULL `value` asked and failed. Created from the Dataset's first refresh, given a row by every append, dropped with the Dataset |

A Dataset refresh reads every file of its files table and copies every
topic into Spiral, so reads never reopen an MCAP file. Adding a file rebuilds
every copied topic, and Spiral keeps the earlier copies. The definition's
`sourceRelation` records the association; ownership is never inferred from
names. The files table declares no primary key. Studio's datasets made before
October 2026 read their files through `dataset_source_<name>` views, which
Spiral retired; their dead entries still hold those names. Curation's published views
are not Spiral resources: Studio keeps them ([curation](curation.md#publishing)).

Publication creates or appends the files table, creates and refreshes the
Dataset, and verifies its member count in one transaction. Publishing the same
recording key with the same bytes (by SHA-256) again is an idempotent retry;
different bytes at that key are refused. Hugging Face import refuses existing
target resources; Rollout recording appends to a validated matching Dataset. Browser uploads carry
bytes, never filesystem paths for the server to open. The committed Spiral
project journal supplies resource kinds, identities, and versions; ADBC table
listings are not the authority for Dataset membership. Studio does not write
to an arbitrary external dataset.

## Recording

A completed Rollout recording becomes one immutable `.mcap` file: the recording
header, stream metadata, joint observations and actions, and H.264 camera
access units. Camera messages use `foxglove.CompressedVideo`; joint and
metadata messages use the `studio.recording` Protobuf schemas
(`crates/robot-connector/recording`). On final stop, the robot connector saves
and syncs the file in its data directory, at
`recordings/<dataset>/<episode-id>.mcap`. The browser then reads it, has Studio
publish it as above through `POST /datasets/{dataset}/recordings/{episode}`,
and only then has the robot connector delete it. A take that fails to reach
Studio stays on the robot connector; Rollout lists waiting takes and stores them on
request. A stop with `keep: false` marks the take discarded; it is stored the
same way.

Pausing ends the current encoder segment without publishing an incomplete
member; resuming keeps the episode's elapsed clock, excluding the pause. Frame
timestamps use capture time, not frame number. Every keyframe carries its
decoder parameters, so seeking starts from the preceding keyframe. Calibration
changes affect future recordings only: a recording keeps the normalized values
captured with it, and angles in degrees need a calibration snapshot.

Rollout reviews the current take from the robot connector's session, never from
files.
`RecordingBuffer` holds only the current session's capture and review, and
camera preview keeps that session's encoded packets, at most 18,000 packets and
64 MiB per camera, until a new capture starts. `RecordingSession` holds the one
open take and the last one kept for review: an operation that changes the take
(start, pause, stop, or closing a take the control loop ended) holds it
throughout, operations never interleave, and a cancelled one leaves the take
as it was. These routes live under `/recording/takes/{id}`, whose IDs name
session captures, not dataset members. Once the session is replaced or the
robot connector restarts, the take is read through Data like any other episode.

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

Training reads a curated view's intervals and uploads each episode's camera
packets and joint values to the Modal training service, which decodes and
resamples them next to the GPU and keeps the checkpoints
([Training](training.md)).
