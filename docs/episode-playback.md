# Episode-first topic inspection and playback

Open **Data → dataset → episode → topic**. A dataset opens its paginated episode
list. An episode opens only topics with messages in that recording, sorted by
name. Each topic is labeled **Video** or **Table** according to its schema:
`foxglove.CompressedVideo` opens the player; other schemas open the bounded Arrow
message table. Breadcrumbs return to the episode's topics or dataset's episode
list, preserving episode pagination. Recording facts and source details live at
the episode level. The catalog lists resources, not synthetic dataset outputs.

There is no camera selector or dataset-wide topic tab. SQL remains available for
cross-episode queries. Leaving a video topic closes its player; opening a video
starts paused at its first available frame. Play/pause and a keyboard-accessible
timeline slider control one camera at a time. Simultaneous cameras and joint plots
are not connected to this clock yet.

## Data and clock

`POST /api/datasets/episode-topics` accepts `EpisodeSelection` and returns topic
names and schemas. It checks membership, then the dataset's topics with native
`read_topic` queries, binding the exact member UUID and selecting only `id` with
`LIMIT 1`. Empty topics are omitted; errors expose **Retry topics**. Returned
coverage is one episode with no claimed message count. Discovery does not fetch
encoded camera payloads into Studio.

Discovery currently performs one presence query per dataset topic, in sequence.
On the connected 52-episode dataset, listing ten topics took over a minute during
desktop verification. This is a remaining performance limitation of the current
lookup path; changing the navigation hierarchy alone does not remove that cost.

`POST /api/datasets/episode-video` accepts `EpisodeTopicRequest`. Storage reads
only that recording using a bound Arrow UUID, orders packets by MCAP log time
and sequence, and returns H.264 Annex B access units. It uses Spiral's existing
`spql.mcap.read_topic` path, with no transcoding or persistent video copy.
The all-episodes query path is unrelated to this read.

Absolute nanoseconds travel as decimal strings. Rust subtracts the episode's
start before converting to the decoder's microsecond timestamps. Opening the
player displays the first available camera frame. Scrubbing chooses the latest
frame at or before the requested time, starting decode at its preceding IDR
keyframe. Keyframes must carry their SPS/PPS parameter sets. Encoded packets
remain in memory; decoded images are retained for the current keyframe interval.

Rust owns the browser playback clock and WebCodecs lifecycle. A seek into another
keyframe interval cancels the previous decode. Dropping the component closes the
decoder and frames; switching episodes mounts a new owner. The clock pauses for
decoding and stops at the episode end. Hiding the workspace, panel, or browser
page pauses playback. Before camera coverage begins, the canvas is empty.

## Bounds and errors

- H.264 only; no codec fallback or synthetic video.
- At most 18,000 packets and 64 MiB of encoded data per camera read.
- A 128 MiB conservative decoded pixel budget per keyframe interval.
- Strictly increasing packet log times and a decodable first keyframe.
- WebCodecs must be available in the browser over HTTPS or localhost.

Missing camera topics or episode timing produce an explicit unavailable message.
An empty selected camera, invalid packets, unsupported decoding, and exceeded
limits produce an explicit error with **Retry video**. They never display another
recording or live camera as a substitute.

## Verification

Rust browser tests verify clock bounds and restart, epoch precision, and
backward/forward keyframe selection. A native WebCodecs test compares decoded
pixels after a keyframe seek with continuous decoding of the same synthetic
stream. The native recording integration test checks topic discovery across
episodes with two, one, and no cameras, and reads each available stream through
the selected-episode video endpoint's storage path. Its 1.6 KiB video fixture was generated with:

```sh
ffmpeg -f lavfi -i 'testsrc2=size=32x32:rate=30' -frames:v 8 \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p \
  -x264-params 'keyint=4:min-keyint=4:scenecut=0:bframes=0:aud=1' \
  -f h264 crates/frontend/src/activities/data/fixtures/seek.h264
```
