# Schema-driven native episode inspection

Open **Data → dataset → episode**. The rail lists a dataset's members as
chronological episodes; opening one fills the workspace: one camera panel per
`foxglove.CompressedVideo` stream in the right-hand column, **Plots** and
**Messages** tabs in the primary tile beside **SQL**, and the **Timeline**
across the bottom. Every panel reads the same episode-owned clock and the same
visible span; native tabs, splits, and resizing remain available.

The Timeline lists every stream with messages in the episode, in topic order.
Ticking a stream shows it: a camera opens its panel; a data stream draws all of
its numeric fields (bounded to 16 per stream). Expanding a data stream reads its schema once and
lists nested structs, lists, scalar types, and nullability (`?`) with
per-field checkboxes. Clicking a stream name selects it for Messages. Kind is
decided from recorded schema names only; topic and field names carry no
physical meaning, and a validity flag is an ordinary selectable boolean.

Plots draw one section per plotted stream and one lane per field, one trace
per recorded array index. The readout column shows each trace's value at the
playhead or under the hover cursor, and doubles as the legend (clicking a
trace hides it). Traces load a complete bounded window; if the whole episode
exceeds the read bounds, that stream falls back to the visible span and says
so. Messages shows the selected stream from the playhead forward, one hundred
at a time, in the shared result table. Video and message previews mount only
while their panel is visible; a closed camera releases its buffers and
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
discovery, message tables, plots and recorded camera packets. There are no
dedicated Data joint, JSON topic-preview, video, or message endpoints.

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

Opening an episode selects the dockable **3D Stage**, using the same viewport
and clock as Studio's other panels. During Hugging Face import, GPT-5.6 Terra
reads the pinned repository and recorded samples, converts a supplied robot
description to URDF when necessary, and resolves its joint bindings. The URDF,
meshes and bindings are saved per project and dataset revision; absent models
stay empty. Import failures leave the episodes available with a summary warning.
Opening an episode only reads that saved result. Existing imports were amended
once; Studio capture is outside this feature. Joint playback uses the existing
message buffer's time lookup, with recorded units and mimic relations preserved.

Studio robots carry their own `visualization` in the project's robot document:
URDF, mesh assets, joint telemetry coordinates, and an optional tool link. Live
and Studio-recorded poses use that configuration. The existing leader and follower
definitions were written to the database once; no SO-ARM101 definition is kept
in the repository or automatically inserted into new robot configurations.

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
The existing time-series chart renderer and message-result component are reused.

Cameras use a media adapter for `foxglove.CompressedVideo` H.264. They consume
binary packets from the same message transport and reuse the Annex B parser.
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

Each HTTP page allows 1–4,096 rows, up to 64 projected fields, and 32 MiB of Arrow
data. A plotted or camera window allows 36,000 rows / 128 MiB of encoded IPC;
exceeding a limit produces an error instead of silently plotting partial data.
The schema UI allows 16 selected fields and 64 resulting scalar/index traces.
Lists exceeding 64 elements require a smaller field. Message previews show the
first 100 rows and indicate continuation.

Visible camera topics load independently and can download concurrently. Retained
camera packets share a 128 MiB budget, and retained decoded frames share 512 MiB
(accounted at four bytes per coded pixel) with a 128 MiB bound per camera; with
playhead-following retention a camera holds about fifteen frames. Accounting follows owned packets and frames, without counting
shared Arrow backing buffers repeatedly. WebCodecs requires HTTPS or localhost
and browser H.264 support. Desktop minimum viewport is 1024 CSS pixels.

## Verification and driver requirement

The regression suites passed 447 native tests and 124 Firefox tests. The
release fullstack build passed. The native MCAP integration also passed, including
the foreign sensor schema, tied-timestamp pagination, exact UInt64 values and
cleanup of its disposable dataset. With the updated driver, it also compares
every projected nested array with the full source message across pages, including
nulls. Desktop checks at 1024 × 768 and 1440 × 900 verified cameras,
scalar/boolean and array traces, absent actions, episode switching and retained
field selection. The four-pane default was also checked at both widths, including
overflow-tab switching and locally scrolling schema/trace content.

Native/browser regressions cover exact values, nested sensor fields, ragged
lists, nulls, tied timestamps, page cursors, projection validation, camera memory
ownership, keyframe decoding, shared playback and project-session isolation.
The disposable native import test verifies cameras and paginated source values;
an additional foreign `example.Environment` producer exercises schema discovery
and projected UInt64 readings without Studio recording DTOs.

Spiral commit `320d50ddb` fixes demand-pruning convergence for nested projections.
Studio's local driver was rebuilt from `xiangpeng/dev` commit `02d06b5c5` and the
verification preview restarted. The demo now loads all 549 messages with position
and action arrays through the normal projected Arrow transport. No Studio
workaround is needed. A source reader can still read a containing struct
internally; selecting a nested field does not guarantee leaf-only storage I/O.

Use the `local-driver` shell with this build. The default Nix shell still selects
the older public archive, and already running servers retain their loaded driver
until restarted. See [database-boundary.md](database-boundary.md) for the exact
source revision, library digest and rebuild command.

Studio pins published `dioxus-workbench` revision
`69cb3a26d7d5f3d0400a989f7e9295a91b965254`, preserving mounted panel state when
neighboring panels close. No local dependency override is required.

## Panel simplification: removal checks (historical)

This study predates the Timeline-led composition above; it is kept for the
transport and ownership checks it records. It was a bounded component-removal
study, not a performance benchmark. Each
removal was checked against the interaction it previously supported. The shared
transport, native docking and existing renderers were held unchanged.

| Removed mechanism | Replacement | Check |
| --- | --- | --- |
| Camera-first split layout | Four type-agnostic panes with schema-related overflow tabs | Video and numeric topics share one clock; native splits remain available. |
| Separate camera and field panel collections | One topic identity and open set | A video's Video/Fields modes occupy the same panel; closing another topic preserves selected fields. |
| Permanent topic chooser panel | Native tabs and one Topics menu | Every topic starts as a tab; the menu activates or reopens topics. |
| Eagerly mounting every camera | Mount video only in visible Video mode | Inactive video tabs release their camera components; visible panes load independently. |

Retained deliberately: a first-visit guard for schema inspection, so unused tabs
do not fetch schemas and visited field selections survive tab/mode changes.
Video buffers are released while hidden and reload on return. There is no
renderer registry, separate layout policy, or hidden-video cache.

The default four-pane layout uses the existing workbench's split and tile nodes.
Schema grouping is a local layout decision; it does not alter message transport,
renderer selection or topic identity.
