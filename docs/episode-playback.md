# Selected-episode camera playback

Open **Data → dataset → Episodes → episode** to preview the first
`foxglove.CompressedVideo` topic in the dataset's sorted camera list. The
player exposes play/pause and a native keyboard-accessible timeline slider.
This first milestone plays one camera. Additional cameras and joint plots are
not connected to this clock yet.

## Data and clock

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

`python3 scripts/check-mcap-playback.py` checks two real episodes through the
HTTP API, including timestamp bounds, H.264 keyframe contents, and an unknown
recording that must return no video. It performs no writes.

Rust browser tests verify clock bounds and restart, epoch precision, and
backward/forward keyframe selection. A native WebCodecs test compares decoded
pixels after a keyframe seek with continuous decoding of the same synthetic
stream. Its 1.6 KiB fixture was generated with:

```sh
ffmpeg -f lavfi -i 'testsrc2=size=32x32:rate=30' -frames:v 8 \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p \
  -x264-params 'keyint=4:min-keyint=4:scenecut=0:bframes=0:aud=1' \
  -f h264 crates/frontend/src/activities/data/fixtures/seek.h264
```
