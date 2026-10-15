# Datasets and recordings

A dataset is a set of ordinary Spiral tables whose names start
`datasets.<id>.`, that Studio writes itself. Studio decodes every source file
when it imports it and stores each topic as rows, so reads never reopen a
source file and never call a Spiral producer. Spiral's own Dataset resource
kind and its MCAP and LeRobot producers and readers are not used.

A dataset has one of two formats, fixed when it is created:

- **MCAP** (`mcap`): each `.mcap` file is one episode. Rollout recording and the
  Hugging Face MCAP import both produce these files; readers never ask which
  one produced an episode.
- **LeRobot** (`lerobot`): a LeRobot v3 repository on Hugging Face, each of its
  episodes one episode and each feature one topic.

```text
Rollout capture on the robot connector → Studio → browser stores the episode ─┐
Hugging Face → .mcap files at a pinned commit ──────────────────────────────────┼→ MCAP import → datasets.<id>.* → Data, Curation, Training
Hugging Face LeRobot v3 repository @ commit → LeRobot import (FFmpeg) ─────────┘
```

## Catalog layout

`<id>` is the dataset's name: 1–96 letters, numbers, `_` or `-`. Every
table sits at the project's root under one name whose parts are joined by dots
(no part holds one), since Spiral's SDK creates tables at the root only;
statements quote the whole name as one identifier, `"datasets.<id>.episodes"`.
Spiral's catalog itself does not group them: Studio finds a dataset's tables
by their names.

| Table | One row per | Holds |
| --- | --- | --- |
| `datasets.<id>.episodes` | episode | the episode's identity and facts (below) |
| `datasets.<id>.topics` | stream | one recorded stream: a topic and the schema its messages use (below) |
| `datasets.<id>.messages.<stream>` | message | the messages of one stream (below) |
| `datasets.<id>.files` | MCAP episode | the episode's source MCAP file: `recording` (its key) and `source`, an `spql.io.Blob` holding its bytes inline, or pointing at its pinned Hugging Face URI. LeRobot datasets have none |
| `datasets.<id>.answers.<column>` | episode asked | one answer column, what one exact model call answered: `episode_id` (UUID, `arrow.uuid`, not null) and `value`, the answer, NULL where the model could not answer. An episode with no row is not asked yet ([curation](curation.md#stored-answers)) |

A dataset exists when its `episodes` table does, and is an MCAP dataset when
it also has `files`; the catalog alone says both, so listing datasets reads no
rows. The catalog (`GET /api/data-catalog`) lists each dataset once, named
`datasets.<id>`, with its format and its identity: its `episodes` table's
resource and version. The version changes exactly when episodes are added, so
what Studio keeps about a dataset version (its robot and joint roles) follows
it.

### Episodes

| Column | Type | |
| --- | --- | --- |
| `id` | UUID (`arrow.uuid`), not null | the episode's identity, a UUIDv7 Studio assigns when it first imports the episode and never changes |
| `key` | text, not null | the episode's key in its source, unique in the dataset: an MCAP file's name (`<episode>.mcap`), or a LeRobot episode's `episode_<index>` with six digits |
| `source_uri` | text | where it was read from: an MCAP file's pinned `hf://datasets/<org>/<repo>@<commit>/<path>`, or a LeRobot repository's root; null for a file Studio holds (`files`) |
| `source_bytes` | uint64 | the MCAP file's size |
| `sha256` | text | the MCAP file's SHA-256, which makes storing the same file again an idempotent retry |
| `episode_index` | int64 | LeRobot's episode index |
| `tasks` | list of text | LeRobot's task texts |
| `message_count` | uint64, not null | messages across every stream |
| `channel_count` | uint32, not null | streams it holds messages of |
| `streams` | list of `{stream, messages}`, not null | each stream it holds messages of, and how many, so what an episode can show is read from its row alone |
| `message_start_time` | uint64, not null | its first message's log time, in nanoseconds |
| `message_end_time` | uint64, not null | its last message's log time |
| `duration_ns` | uint64, not null | `message_end_time - message_start_time` |

A LeRobot episode has no wall clock: its frames are timed from its start, so
`message_start_time` is 0.

### Topics

| Column | Type | |
| --- | --- | --- |
| `stream` | text, not null | the stream's table under `messages`: the topic in lowercase letters, digits and `_`, then `_` and eight hex digits of the SHA-256 of the topic, the schema name and the value's Arrow type |
| `topic` | text, not null | the topic, as recorded |
| `schema_name` | text | the native schema name (`foxglove.CompressedVideo`, `studio.recording.JointSample`); null for a LeRobot feature |
| `message_encoding` | text, not null | `protobuf` or `json` for MCAP; `parquet` for a LeRobot feature, `h264` for a LeRobot camera |
| `episodes` | uint64, not null | episodes holding at least one of its messages |
| `messages` | uint64, not null | its messages |

A stream is camera video when its schema is `foxglove.CompressedVideo` or its
encoding is `h264`. A topic recorded with two schemas, or one schema whose
Arrow type changed, is two streams, never one coerced into the other.

### Messages

| Column | Type | |
| --- | --- | --- |
| `id` | UUID, not null | the episode |
| `log_time` | uint64, not null | MCAP log time, or the LeRobot frame's time from the episode's start, in nanoseconds |
| `publish_time` | uint64, not null | MCAP publish time (LeRobot: `log_time`) |
| `sequence` | uint32, not null | MCAP sequence (LeRobot: the frame index) |
| `ordinal` | uint64, not null | the message's place in its episode's stream, from 0: file order for MCAP, frame order for LeRobot. Video packets decode in `ordinal` order |
| `value` | the stream's type, not null | the decoded message |
| `keyframe` | bool, not null | video streams only: whether the access unit holds an IDR picture, so a read can start at the keyframe before an instant |

A Protobuf message becomes a struct with a field per message field: a
message-typed field or a field with explicit presence (`optional`, proto2,
oneof members) is nullable; other scalars are not; a repeated field is a
non-null list of non-null items; `string` is `Utf8View`, `bytes`
`BinaryView`, enums their number (`int32`). A JSON message is its raw bytes
(`BinaryView`). Other message encodings are refused. A LeRobot feature with
`names` is a struct of one float field per name; one without is
`{data: fixed_size_list<float>[n]}`; a camera is `{data: BinaryView, format:
Utf8View}` holding one H.264 access unit, `format` `h264`.

## Writing

Imports write episodes in batches of up to 32, each batch in one Spiral
transaction: every stream's message rows first, then the episodes' rows (and
an MCAP episode's file), and last the `topics` table replaced with the updated
counts, once per transaction, since Spiral refuses to replace a table the same
transaction already wrote. A reader that finds an episode therefore finds all
its messages. A failed batch commits nothing; the batches before it stay.

Writing episodes never touches the answer columns: a new episode has no row
in any of them, which reads as not asked yet. Only fill runs write answers,
and only by appending rows ([curation](curation.md#stored-answers)).

- **Recording.** A Rollout episode is one batch of one episode: its file goes
  to `files`, its messages to their streams. Storing the same key with the
  same bytes (by SHA-256) again is an idempotent retry; different bytes at
  that key are refused.
- **Hugging Face MCAP import** takes a public repository's `.mcap` files at a
  pinned commit, downloads and decodes them 32 at a time, and records each
  file's pinned URI in `files` and `source_uri`.
- **Hugging Face LeRobot import** reads `meta/info.json`, the episodes and
  tasks tables and the frame tables, and makes every feature a stream and
  every camera an `h264` stream. Each camera's video file is copied to local
  disk once, while the episodes being read lie in it: a file holds hundreds
  of episodes, and seeking into it over HTTPS fetched its index again for
  each one (2.6 s against 0.04 s locally). FFmpeg (`SPIRAL_ROBOTICS_FFMPEG`,
  default `ffmpeg` on `PATH`) cuts each episode's frames out of the copy, all
  of an episode's cameras at once, and encodes them again as H.264 without
  B-frames, one access unit per frame, a keyframe every 30 frames, the
  parameter sets repeated at every keyframe, so every episode decodes from its
  first frame whatever codec (H.264 or AV1) and keyframe spacing the
  repository used.

Both imports read the next batch while the one before it commits, so
downloads and FFmpeg overlap Spiral's writes.

Both Hugging Face imports refuse an existing dataset name and run as a held
run the browser polls (`POST /api/datasets/imports` with the format and the
source, then `GET /api/datasets/imports/{id}`), reporting the episodes
written so far. The run outlives the page; once it has ended, nobody reading
it for 30 seconds drops it. A restarted backend loses the run; the batches it
committed stay, and a new import under the same name is refused, so delete
the dataset first. Downloads send the deployment's `HF_TOKEN` when it is set,
and the repository must be public. Rollout recording appends only to an
MCAP dataset. Browser uploads carry bytes, never filesystem paths for the
server to open. Deleting a dataset drops every table under `datasets.<id>`.

## LeRobot datasets

A LeRobot episode is labeled `Episode <index>`, ordered by `episode_index`,
and has no files, size or channel summary, so a Select offers only the facts
it records: duration and message count. Every feature is a stream of every
episode, one message per frame, at the frame's time, except the columns that
place a frame (`timestamp`, `frame_index`, `episode_index`, `index`,
`task_index`): they become each message's position and the episode's row.
Image features and non-numeric features are refused.

## Recording

A completed Rollout recording becomes one immutable `.mcap` file: the recording
header, stream metadata, joint observations and actions, and H.264 camera
access units. Camera messages use `foxglove.CompressedVideo`; joint and
metadata messages use the `studio.recording` Protobuf schemas
(`robot-connector/recording`). The robot connector writes the file in its
data directory while the episode records, one thread taking joint samples and
camera access units as they arrive, so an episode of any length holds little in
memory. Stopping writes the messages that sum the episode up (the episode
header and the stream metadata, last in the file; readers place messages by
log time), saves it at `recordings/<dataset>/<episode-id>.mcap`, and uploads
it to Studio part by part. Once Studio holds it whole, the connector keeps it
only for review, until the next episode opens. Studio keeps the episode until a
browser stores it as above with its user's Spiral access: Rollout does so
as each episode arrives, and lists an episode whose store failed, to try again. A
stop with `keep: false` marks the episode discarded; it is stored the same way.

Pausing ends the current encoder segment and puts everything written on
disk, so the paused episode's file ends at a whole chunk; resuming keeps the
episode's elapsed clock, excluding the pause. Frame
timestamps use capture time, not frame number. Every keyframe carries its
decoder parameters, so seeking starts from the preceding keyframe. Calibration
changes affect future recordings only: a recording keeps the normalized values
captured with it, and angles in degrees need a calibration snapshot.

Rollout reviews the paused or last episode from its file on the robot connector,
read a chunk at a time: the joint positions for the Timeline, and one
camera's packets, at most 18,000 and 64 MiB, for its preview.
`RecordingBuffer` holds only the last episode's summary. `RecordingSession` holds
the one open episode and its file: an operation that changes the episode
(start, pause, stop, or closing an episode the control loop ended) holds it
throughout, operations never interleave, and a cancelled one leaves the episode
as it was. These routes live under `/recording/episodes/{id}`, whose IDs name
session captures, not dataset episodes. Once the session is replaced or the
robot connector restarts, the episode is read through Data like any other episode.

Automated tests never move hardware; physical capture still needs a smoke
test with a real robot and camera.

## Reading episodes

Readers use the dataset's tables, never a source file: `episodes` for a
population and, through its `streams`, what an episode can show; `topics`
for the dataset's streams; and `messages.<stream>` for a stream, filtered to
the episode by `id`. `DatasetSelection` (in
`common`) names every table, so no statement spells a table name itself.
Episode-scoped reads bind the episode UUID as `$1`; no filename, timestamp,
text-cast, or raw-storage filter substitutes for episode identity. They learn
what a stream holds from its schema and never require Studio's own topics or
capture types; the metadata Studio writes into its files is ordinary optional
content. [Episode playback](episode-playback.md) describes the inspection
built on them.

Project bundles carry each MCAP dataset's name and its `files` rows as their
Blobs hold them, the bytes of a file held inline or the pinned URI of one
imported from Hugging Face, including empty datasets; import drops the
project's MCAP datasets and imports the bundle's files again, downloading each
pinned one at its commit, in the same transaction as Studio state, so restored
episodes get new identities and a pinned dataset keeps pointing at its URIs.
A bundle naming a dataset the project holds as a
LeRobot dataset is refused before anything changes. A LeRobot dataset stays in its project and is
imported again from Hugging Face. See
[project transfer](studio-sqlite.md#project-transfer).

## Training and rollout

Training reads a curated view's intervals and uploads each episode's camera
packets and joint values to the Modal training service, which decodes and
resamples them next to the GPU and keeps the checkpoints
([Training](training.md)).
