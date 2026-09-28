# Schema-driven native episode inspection

Open **Data → dataset → episode**. The rail lists a dataset's members as
chronological episodes; opening one shows the episode inspection Curation
uses too: one camera panel per `foxglove.CompressedVideo` stream, a **Scene**
tile with **Plots** behind it, and the **Timeline** across the bottom. Every
panel reads the same episode-owned clock and the same visible span; native
tabs, splits, and resizing remain available, and each dataset remembers its
arrangement in the browser.

The Timeline lists every stream with messages in the episode, in topic order.
Ticking a stream shows it: a camera opens its panel; a data stream draws all of
its numeric fields (bounded to 16 per stream). Expanding a data stream reads its schema once and
lists nested structs, lists, scalar types, and nullability (`?`) with
per-field checkboxes. Kind is decided from recorded schema names only; topic and field names carry no
physical meaning, and a validity flag is an ordinary selectable boolean.

Plots draw one section per plotted stream and one lane per field, one trace
per recorded array index; the playhead and the hover cursor cross every lane.
Traces load a complete bounded window; if the whole episode exceeds the read
bounds, that stream falls back to the visible span and says so. Video mounts
only while its panel is visible; a closed camera releases its buffers and
reloads when shown again.

## One message transport

`POST /api/datasets/episode-topics` reads the selected member's native
`spql.mcap.channels` summary and joins topic identities to native catalog
observations. Incompatible schema variants remain distinct. Each source carries
its native schema variant; requests bind the member UUID.

The browser reads messages with ordinary episode SQL. `MessageRead` (in
`common`) writes the `spql.mcap.read_topic` query for one topic revision, and
`POST /api/queries/episode/arrow` runs it with `$1` bound to the member UUID and
returns `application/vnd.apache.arrow.stream`. The same query serves schema
discovery, plots and recorded camera packets; there are no dedicated joint,
video, or message endpoints.

- A schema read is the same query with `LIMIT 0`: the native Arrow schema,
  including nested types, field metadata and nullability, without message rows.
- A window specifies a half-open nanosecond interval, structured field paths,
  a row limit, and an optional continuation cursor.
- Pages retain `log_time`, `publish_time`, `channel_id`, `chunk_start_offset` and `sample_ordinal`.
  Projection aliases correspond to the ordered requested paths.
- Ordering and continuation use `(log_time, chunk_start_offset, sample_ordinal)` so messages with
  equal timestamps across chunks are not skipped. Each page asks for one row past its
  limit; that row means the browser continues after the last kept message.
- The Arrow endpoint returns the complete result or refuses it (over 10,000 rows
  or 32 MiB); it never truncates. Reads are pinned to the member UUID, so a new
  take appended to the dataset does not disturb an open episode. The browser
  rejects a schema that changes between pages.

Arrow retains exact integers, timestamps, nulls and binary packets. Plot geometry
uses floating point; current-value text is formatted from the original Arrow
value, retaining 64-bit integer precision. Timestamp subtraction happens in Rust
before conversion to seconds.

## Plots and playback

The **Scene** draws the dataset's robot with the same viewport and clock as
Studio's other panels. During Hugging Face import, the configured model reads
the pinned repository and recorded samples, converts a supplied robot
description to URDF when necessary, and resolves its joint bindings. The URDF,
meshes and bindings are saved per project and dataset revision; absent models
stay empty. Import failures leave the episodes available with a summary warning.
Opening an episode only reads that saved result. Joint playback uses the
message buffer's time lookup, with recorded units and mimic relations preserved.

Studio robots carry their own `visualization` in the project's robot document:
URDF, mesh assets, joint telemetry coordinates, and an optional tool link. Live
and Studio-recorded poses use that configuration. No SO-ARM101 definition is
kept in the repository or inserted into new robot configurations.

Selecting fields loads a complete bounded time window, following every page.
Changing the window or selection replaces that read. Playback uses the loaded
samples and does not fetch on each clock tick. Current values select the latest
message at or before the playhead within the chosen window; sample time and age
are explicit. There is no inferred interpolation or sample-index gap rule.
Null numeric values break traces; changing array lengths leave missing indices
empty. Names containing punctuation remain distinct from nested field paths.

Data, Curation, and Live compose the same controlled component in
[`components/timeline`](../crates/frontend/src/components/timeline/mod.rs).
`TimelineInteraction` binds a caller-owned clock projection to typed events;
`TimelineContent` supplies rows, identity, toolbar additions, and notices.
The component shares transport controls, ruler, keyboard/wheel handling,
scrubbing, numeric and interval tracks, and playback timing. Each activity
owns its effects: Data selects streams, Curation handles interval activation,
and Live controls recording and permits robot motion only on committed seeks.
Events carry the source identity; owners reject events from replaced sources.

Cameras use a media adapter for `foxglove.CompressedVideo` H.264. They consume
binary packets from the same message transport.
Each visible camera keeps one WebCodecs decoder following the playhead: forward
playback decodes every frame once, a jump restarts from the preceding keyframe,
and only frames within a quarter second behind the playhead plus a short
lookahead stay in memory, so every camera of an episode can be open at once.
Seeking deep into a long keyframe interval reports buffering so the shared clock
waits for the picture. Camera timestamps and age remain
visible; uncovered times, unsupported codecs and decoding errors are explicit.
Closing panels releases their media. Hiding the activity, dataset panel, or
browser pauses playback. Space toggles playback outside interactive controls.

## Bounds

Each message page allows 1–4,096 rows and up to 64 projected fields; the Arrow
endpoint refuses a result over 10,000 rows or 32 MiB. A plotted or camera
window allows 36,000 rows and 128 MiB of encoded IPC; exceeding a limit
produces an error instead of silently plotting partial data. A stream plots at
most 16 fields, and a plotted list over 64 elements asks for a smaller field. A
keyframe interval whose decoded frames exceed 128 MiB reports an error instead
of exhausting the tab. WebCodecs requires HTTPS or localhost and browser H.264
support. Desktop minimum viewport is 1024 CSS pixels.

## Driver and workbench

Use the `local-driver` shell: the default Nix shell's public archive predates
the native MCAP query functions, and a running server keeps the driver it
loaded until restarted. See [database-boundary.md](database-boundary.md) for
the rebuild command.

Studio uses the published `dioxus-workbench` 0.1.2 from crates.io, which
preserves mounted panel state when neighboring panels close. No local
dependency override is required.
