# Recording

## Outcome

Recording captures leader-to-follower teleoperation, joint state, commands, and
assigned cameras as one complete episode in the selected dataset. It is the
built-in producer for the data core; external data enters through
[ingestion](../data/ingestion.md).

## User flow

1. The user chooses Record in Live.
2. Studio enters Teleop and starts one episode.
3. The Timeline follows captured data.
4. The user may pause, resume, stop and keep, or discard.
5. A kept recording returns to Live and briefly reports that it was saved to
   Data.
6. The backend commits the complete episode directly to SpiralDB.

## Recording controls

- **RECORD-001** Recording controls must appear in Live before manual control.
- **RECORD-002** Record must start leader-to-follower Teleop and one episode.
- **RECORD-003** A recording must support pause, resume, stop, and discard.
- **RECORD-004** Pause and resume must preserve the same episode identity.
- **RECORD-005** Pause must create an explicit gap in the episode clock.
- **RECORD-006** A motor failure that interrupts recording must not create a
  silent time hole.
- **RECORD-007** Stopping and keeping must return to Live instead of opening the
  new episode.
- **RECORD-008** After a kept recording stops, Studio must show a brief stable
  cue that the episode was saved to Data.
- **RECORD-009** A discarded or incomplete episode must not appear as usable
  data.

## Episode contract

- **RECORD-010** An episode must keep one identity through start, pause, resume,
  completion, or discard.
- **RECORD-011** An episode must declare its dataset, source, sample rate, start
  time, robots, and ordered joints.
- **RECORD-012** Joint state, commanded action, camera media, and gaps must remain
  separate streams on one clock.
- **RECORD-013** Every sample time must be finite, non-negative, and monotonic.
- **RECORD-014** Every joint sample must match the declared robot and joint
  shape.
- **RECORD-015** Camera segments must declare stream identity, clock offset,
  dimensions, frame rate, and frame count.
- **RECORD-016** A kept episode must commit as one complete unit.

## Persistence

- **RECORD-017** Every completed kept episode must commit directly to SpiralDB
  without a local durable staging copy.
- **RECORD-018** Studio must not expose Push, Sync, or Retry states that imply a
  second storage layer.
- **RECORD-019** A failed SpiralDB commit must be reported as a recording error;
  Studio must not silently fall back to local durable storage.
- **RECORD-020** Persistence must not depend on an open browser session.
- **RECORD-021** Pausing must checkpoint accepted joint samples and completed
  camera segments directly to SpiralDB so paused inspection is deterministic.
- **RECORD-022** One backend recording-session manager must own the catalog
  reservation, episode identity, opening/recording/finalizing phase, and
  optional camera capture through durable commit. Hardware remains the sole
  authority for physical sampling and motor state; the catalog lease and HTTP
  application state must not carry independent recording-session copies.

The durable write behavior is defined by [Data persistence](../data/persistence.md).
Recorded inspection is defined by
[Timeline and replay](../data/timeline-and-replay.md).

## Implementation ownership

- Recording use case:
  [`spiral-robotics-backend/src/recording.rs`](../../crates/spiral-robotics-backend/src/recording.rs)
- Recording session lifecycle:
  [`spiral-robotics-backend/src/recording_session.rs`](../../crates/spiral-robotics-backend/src/recording_session.rs)
- Hardware capture:
  [`spiral-robotics-hardware`](../../crates/spiral-robotics-hardware/)
  Hardware receives only a durable completion token after storage commits; it
  does not duplicate the episode wire summary. Hardware-native samples and
  condition measurements cross the capture-sink trait without `serde` or a
  storage schema; the storage adapter owns their durable projection.
- Episode and media adapters:
  [`backend/recording`](../../crates/spiral-robotics-backend/src/recording/)
- Live recording UI:
  [`frontend/activities/live`](../../crates/spiral-robotics-frontend/src/activities/live/)
- Episode invariants:
  [`spiral-robotics-api/src/recording.rs`](../../crates/spiral-robotics-api/src/recording.rs)
  and
  [`spiral-robotics-storage/src/episodes.rs`](../../crates/spiral-robotics-storage/src/episodes.rs)
