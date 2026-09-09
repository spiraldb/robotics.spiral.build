# MCAP recording and playback

Use the current MCAP-capable native Spiral driver. This checkout's default
pinned wheel predates the MCAP query API; the existing local-driver shell loads
`target/release/libadbc_driver_spiraldb.dylib` on macOS (the `.so` on Linux):

```sh
nix develop .#local-driver
cd crates/studio
dx serve --release --fullstack --debug-symbols false
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
If import fails, the error includes the durable file path. There is currently no
restart recovery/retry UI for these local files.

Pausing ends the current encoder segment without publishing an incomplete
Dataset member. Resuming preserves the episode's elapsed clock, excluding the
pause. Frame timestamps use capture time, not nominal frame number. Every
keyframe carries its decoder parameters; seeking starts from the preceding
keyframe. Data plays a selected camera topic; Live's recorded cameras and joint
plots use the same episode playhead. Decoding uses browser WebCodecs through
Rust/WASM bindings. There is no server-side MCAP-to-training decoder workaround.

Saved files remain immutable. Calibration changes affect future recordings;
inspection uses the normalized values recorded at capture time. Historical
angles in degrees are unavailable without a calibration snapshot. Deleting an
episode removes its Dataset source row and refreshes the native Dataset, but
does not erase the retained local MCAP file.

Project bundle format 2 includes original MCAP bytes and Dataset names, including
empty datasets. Import rebuilds their filtered views and native populations in
the same transaction as Studio state. It validates ownership before replacing
resources. The portable project file limit remains 256 MiB. Playback requests
are bounded to 18,000 packets and 64 MiB of encoded video per camera; oversized
requests report an error. Existing format-1 backups require the old application.

MCAP camera training is explicitly unavailable until Spiral exposes native
MCAP video decoding for training images. Training history and existing
checkpoints remain accessible; the old registry/MP4 training loader is removed.
The separate model inference and Arrow transport runtime remains available,
subject to its existing exact checkpoint-schema compatibility check. Schema-10
checkpoints remain inspectable/downloadable; this schema-11 migration does not
make them compatible with rollout.

Validation covers MCAP joint/null/action round trips, camera timestamps,
immutable file publication, pause timing, real FFmpeg keyframe packaging, and
native Spiral import/retry/playback/delete. Hardware capture still needs a real
robot/camera smoke test; automated validation does not move hardware.

The migration suites pass: 264 backend, 110 storage, 105 hardware, 60 common,
5 training-protocol, 115 browser/WASM, and 14 Python tests, plus the native
Spiral import/retry/playback/delete and FFmpeg keyframe tests. Seven separate
frontend source-architecture assertions still fail in `activity_composition`
and `interaction_language`; the same seven failures were reproduced from an
unchanged `HEAD` snapshot. They are not counted as passing migration checks.
