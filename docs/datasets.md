# Datasets and recordings

A dataset is a native Spiral Dataset. Studio makes two kinds, by the Spiral
producer that turns their source files into episodes:

- **MCAP recordings** (`spql.mcap.dataset`): each `.mcap` file is one episode.
  Rollout recording and the Hugging Face MCAP import both produce these files;
  readers never ask which one produced an episode.
- **LeRobot datasets** (`spql.lerobot.dataset`): a LeRobot v3 repository on
  Hugging Face, each of its episodes one member and each feature one topic.

```text
Rollout capture on the robot connector → browser → complete MCAP bytes ─┐
                                                                    ├→ dataset_files_<name> (Table) ──→ dataset_<name> → Data, Curation, Training
Hugging Face → pinned Blob URIs of .mcap files ─────────────────────┘
Hugging Face LeRobot v3 repository @ commit → dataset_files_<name> (Location) → dataset_<name>
```

Readers decide what an episode lists by the dataset's producer, which the
dataset's definition records and `POST /api/datasets/inspect` reports
(`producer`), never by guessing per row.

## Catalog layout

The selected Spiral project holds Studio's resources at its root:

| Resource | Kind | Purpose |
| --- | --- | --- |
| `dataset_files_<name>` | Table, or Location | The files the Dataset is made from. MCAP: a table of `recording` (an opaque source key within its dataset, not an episode identity) and the `source` Blob. LeRobot: a Location over `hf://datasets/<org>/<repo>@<commit>/`, whose rows are the repository's files (`path`, `file` Blob), listed by Spiral and never copied |
| `dataset_<name>` | Native Dataset | Made from its files through `spql.mcap.dataset` (`recording` the member key) or `spql.lerobot.dataset` (`path` the key, `file` the Blob), with `materialization = 'all'` |
| `dataset_answers_<name>` | Table | One row per episode, keyed by `episode_id`, and one column per exact model call a curation made, each value `STRUCT(value …)`: NULL not asked yet, a NULL `value` asked and failed. Created from the Dataset's first refresh, given a row by every append, dropped with the Dataset |

A Dataset refresh reads every file and copies every topic into Spiral, so
reads never reopen a source file. Adding an MCAP file rebuilds every copied
topic, and Spiral keeps the earlier copies. A LeRobot refresh also decodes
each camera's video and encodes it again as H.264 packets, in groups of one
second, which `spql.mcap.read_video` reads like recorded MCAP cameras. The
definition's `sourceRelation` records the association; ownership is never
inferred from names. The files table declares no primary key. Studio's
datasets made before October 2026 read their files through
`dataset_source_<name>` views, which Spiral retired; their dead entries still
hold those names. Curation's published views are not Spiral resources: Studio
keeps them ([curation](curation.md#publishing)).

Publication creates or appends the MCAP files table, creates and refreshes the
Dataset, and verifies its member count in one transaction. Publishing the same
recording key with the same bytes (by SHA-256) again is an idempotent retry;
different bytes at that key are refused. Both Hugging Face imports refuse
existing target resources; Rollout recording appends to a validated matching
MCAP Dataset, and its destination field offers only datasets made from their
own files table, never a LeRobot dataset. Browser uploads carry bytes, never
filesystem paths for the server to open. The committed Spiral project
journal supplies resource kinds, identities, and versions; ADBC table
listings are not the authority for Dataset membership. Studio does not write
to an arbitrary external dataset. Project bundles carry MCAP datasets only; a
LeRobot dataset stays in its project and is imported again from Hugging Face.

## Hugging Face import

**Data → Import** takes a public Hugging Face repository (`org/repo`), a
revision, and the dataset name, and what the repository holds: MCAP
recordings or a LeRobot dataset. Inspect resolves the revision to its commit
(a branch moves; a commit hash pins the import), lists the files it reads and
their size, and names the relations it creates; the import reads that
commit. No token is sent: the repository must be public.

- **MCAP recordings** import in one request: the files table of the `.mcap`
  files' pinned Blob URIs, the Dataset, its refresh, and its answers table,
  in one transaction.
- **A LeRobot dataset** imports as a run the backend holds
  (`POST /api/datasets/lerobot/imports`) and the browser polls every two
  seconds (`GET /api/datasets/lerobot/imports/{id}`). The run, in one
  transaction on a shared connection, creates the Location
  `dataset_files_<name>` over `hf://datasets/<org>/<repo>@<commit>/`, refreshes
  it, creates the Dataset with `spql.lerobot.dataset` over it, refreshes the
  Dataset, and creates the answers table, reporting each phase: listing files,
  creating the dataset, copying episodes (the long one: Spiral reads every
  episode and encodes its cameras again, in the backend's process, so the
  driver's FFmpeg must decode AV1 and encode H.264), creating the answers
  table. Spiral's error ends the run as it said it, and nothing is committed;
  a restarted backend loses the run and commits nothing either. The run
  outlives the page; once it has ended, nobody reading it for 30 seconds drops
  it. Creating and listing a Location needs the `locations:read` and
  `locations:write` scopes Studio asks for at sign-in; a session signed in
  before Studio asked for them must sign in again.

## LeRobot datasets

Spiral lists a LeRobot episode in `spql.mcap.episodes` with its
`episode_index`, `tasks` (the task text), `message_count`, and its span
(`message_start_time`, `duration_ns`), its frames' times from the episode's
start, so it has no wall clock. Its `source` is the frame table it shares with
other episodes, so Studio lists no file, size or channel count for it, labels
it `Episode <index>`, orders episodes by index, and a Select offers only the
facts it records: duration and message count.

Every feature is a topic of every episode, one message per frame, at the
frame's time: a named vector (`observation.state`, `action`) is a struct with
one field per name (`shoulder_pan.pos`, …, or `left_joint1`, …), another
feature a `data` field, and a camera (`observation.images.*`) H.264 packets in
`data` with their `format`. A topic records no native schema name; Studio
knows a camera by the message encoding its producer recorded for it (`mp4`).
Spiral keeps no channel summary of a LeRobot member, so an episode's topics
are the dataset's topics it holds messages of, counted with
`spql.mcap.read_topic` in one statement.

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

Readers use dataset identity and version, its producer, member UUID, topic
and schema revision, timestamps, and values, and learn what a stream holds
from its schema. They never read the source table, reconstruct filenames, or
require Studio's own topics or capture types; the metadata Studio writes into
its files is ordinary optional content. `spql.mcap.episodes('dataset_default')`
reads a population and `spql.mcap.read_topic('dataset_default',
'/studio/joints/leader')` reads a topic. [Episode playback](episode-playback.md)
describes the inspection built on them.

Episode-scoped reads bind the member UUID as `$1`. The driver must compare the
UUIDv7 in member metadata with the generic UUID that Arrow parameter binding
restores (Spiral PR #7033, `ed42714c9`); no filename, timestamp, text-cast, or
raw-storage filter substitutes for member identity.

Project bundles carry the original MCAP bytes and Dataset names, including
empty datasets; import validates ownership and rebuilds the views and
populations in the same transaction as Studio state. See
[project transfer](studio-sqlite.md#project-transfer).

## Training and rollout

Training reads a curated view's intervals and uploads each episode's camera
packets and joint values to the Modal training service, which decodes and
resamples them next to the GPU and keeps the checkpoints
([Training](training.md)).
