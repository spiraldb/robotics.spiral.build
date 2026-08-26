# Timeline and replay

## Outcome

The Timeline provides one time authority for live capture and recorded
inspection. Replay and Sync robot apply validated recorded actions to the
follower without making visual scrubbing dangerous.

## Recorded inspection

- **TIMELINE-001** The Timeline must control live capture, recorded signals,
  camera history, 3D pose, and replay through one clock.
- **TIMELINE-002** Selecting an episode must bind the Scene, camera panels,
  signals, and Timeline to that episode.
- **TIMELINE-003** Recorded views must derive state from the selected episode and
  playhead, not the previously open route.
- **TIMELINE-004** Moving the playhead must update camera panels and 3D pose for
  the selected time.
- **TIMELINE-005** A camera frame must use its stream frame rate and episode
  offset.
- **TIMELINE-006** Camera frame index must not be derived from joint sample rate
  when the rates differ.
- **TIMELINE-007** The Timeline must not duplicate camera imagery as a camera
  row.
- **TIMELINE-008** Play after pause must continue from the current playhead.
- **TIMELINE-009** A separate Rewind action is not required.
- **TIMELINE-010** The first visual inspection of an episode in a browser
  session must load one immutable inspection snapshot containing the episode
  clock, joint state tracks, signal inputs, and video alignment metadata.
- **TIMELINE-011** The inspection snapshot must store frame data in shared
  columnar tracks so arm and joint identity are not repeated for every sample.
- **TIMELINE-012** Scene pose, signal charts, preview playback, and visual
  scrubbing must resolve from the loaded snapshot without making per-frame
  database requests.
- **TIMELINE-013** Studio must retain a bounded browser cache of inspection
  snapshots, distinguish a changed persisted frame count, and invalidate an
  episode on explicit reload or recording lifecycle changes.
- **TIMELINE-014** Video payloads must remain separate range-addressable media;
  the inspection snapshot carries only the metadata needed to align them with
  the episode clock.
- **TIMELINE-015** The recorded-video HTTP endpoint must translate one valid
  browser `Range` request into `spql.io.read_bytes(blob, offset, size)` and
  forward exactly those bytes with the corresponding HTTP range headers. It
  must not materialize the complete MP4 before slicing it or introduce an
  backend payload cache to compensate for Binary storage.

## Physical replay

- **REPLAY-001** Before motion, the backend must validate the complete action
  trajectory, exact joint set, calibration, and sample clock.
- **REPLAY-002** Replay must target a compatible calibrated follower.
- **REPLAY-003** Replay must ramp the follower to the first action before normal
  playback.
- **REPLAY-004** Pause and resume must continue from the current replay frame.
- **REPLAY-005** Sync robot must be one explicit option that applies a committed
  playhead seek to the follower.
- **REPLAY-006** Visual scrubbing must not move a physical arm unless Sync robot
  is enabled and backend validation passes.
- **REPLAY-007** Physical sync must apply on committed release, not every pointer
  event.
- **REPLAY-008** Missing, corrupt, non-finite, or incompatible data must fail
  closed before motion.

## Implementation ownership

- Timeline panel and state:
  [`frontend/panels/timeline`](../../crates/spiral-robotics-frontend/src/panels/timeline/)
- Recorded camera panel:
  [`frontend/panels/camera.rs`](../../crates/spiral-robotics-frontend/src/panels/camera.rs)
- Seek and replay policy:
  [`spiral-robotics-backend/src/data/seek.rs`](../../crates/spiral-robotics-backend/src/data/seek.rs)
  and
  [`spiral-robotics-backend/src/robots/control.rs`](../../crates/spiral-robotics-backend/src/robots/control.rs)
- Hardware replay:
  [`spiral-robotics-hardware`](../../crates/spiral-robotics-hardware/)
