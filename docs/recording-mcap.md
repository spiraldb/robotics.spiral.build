# MCAP recording and playback

Use the current MCAP-capable native Spiral driver. This checkout's default
pinned wheel predates the MCAP query API; the existing local-driver shell loads
`target/release/libadbc_driver_spiraldb.dylib` on macOS (the `.so` on Linux):

```sh
nix develop .#local-driver
dx serve --release --fullstack --debug-symbols false \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

A completed Live recording becomes one immutable `.mcap` file containing the
recording header, stream metadata, joint observations/actions, and H.264 camera
access units. Camera messages use `foxglove.CompressedVideo`; joint and metadata
messages use the existing `studio.recording` Protobuf schemas. No MP4 recording
container or generated video Dataset is involved.

On final stop, Studio saves and syncs the file under the platform local-data
folder: `spiral-robotics/recordings/<project-hash>/<dataset>/<episode-id>.mcap`.
It then imports the bytes as a native Blob in `dataset_source`, creates the
filtered `dataset_source_<name>` view and `dataset_<name>` Dataset when needed,
and refreshes the Dataset in one transaction. Reimporting identical bytes with
the same recording key is idempotent; different bytes at that key are rejected.
Publication uses the same MCAP dataset definition and membership validation as
external-file import; it does not require Studio metadata. If import fails, the
error includes the durable file path. There is currently no
restart recovery/retry UI for these local files.

Pausing ends the current encoder segment without publishing an incomplete
Dataset member. Resuming preserves the episode's elapsed clock, excluding the
pause. Frame timestamps use capture time, not nominal frame number. Every
keyframe carries its decoder parameters; seeking starts from the preceding
keyframe. Data reads published members through dataset/topic APIs. Live’s camera and joint
review uses its current-session capture buffer and one take playhead. It never
reopens source files or looks up a published member through a recording header. Decoding uses browser WebCodecs through
Rust/WASM bindings. There is no server-side MCAP-to-training decoder workaround.

Saved files remain immutable. Calibration changes affect future recordings;
inspection uses the normalized values recorded at capture time. Historical
angles in degrees are unavailable without a calibration snapshot. Deleting the current take
removes its source key from its explicit destination and refreshes that dataset, but
does not erase the retained local MCAP file.

Project bundle format 2 includes original MCAP bytes and Dataset names, including
empty datasets. Import rebuilds their filtered views and native populations in
the same transaction as Studio state. It validates ownership before replacing
resources. The portable project file limit remains 256 MiB. Playback requests
are bounded to 18,000 packets and 64 MiB of encoded video per camera; oversized
requests report an error. Existing format-1 backups require the old application.

MCAP camera training is explicitly unavailable until Spiral exposes native
MCAP video decoding for training images. Existing checkpoints remain listed
and loadable; the old registry/MP4 training loader is removed.
The separate model inference and Arrow transport runtime remains available,
subject to its existing exact checkpoint-schema compatibility check. Schema-10
checkpoints remain inspectable/downloadable; this schema-11 migration does not
make them compatible with rollout.

Validation covers published MCAP joint/null/action values, camera timestamps,
immutable file publication, pause timing, real FFmpeg keyframe packaging, and
native Spiral import/retry/playback/delete. Hardware capture still needs a real
robot/camera smoke test; automated validation does not move hardware.

See [Dataset and recording boundary](dataset-boundary.md) for producer ownership,
session-review lifetime, and the provenance-independent read contract.
