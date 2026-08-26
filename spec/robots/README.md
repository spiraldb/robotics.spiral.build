# Robots

## Outcome

The Robots feature creates and maintains one safe, usable robot instance for
each SO-ARM101 arm.

## Feature map

- [Setup and calibration](setup.md)
- [Live control and safety](control.md)

## Shared requirements

- **ROBOT-001** One robot instance must represent one physical arm.
- **ROBOT-002** SO-ARM101 leader and follower arms are the only current hardware
  family.
- **ROBOT-003** A robot instance must identify its role, port, motors, camera
  assignments, and calibration.
- **ROBOT-004** The backend must try to reconnect configured robots when it
  starts.
- **ROBOT-005** The browser must never drive a robot without the Studio backend.
- **ROBOT-006** Studio must open physical motor buses only; product simulation
  is deferred until it has a separate, principled design.

## Data and boundaries

Robot definitions, motors, camera assignments, and calibrations are durable
project data. Their production persistence is specified by
[Data persistence](../data/persistence.md). JSON is an import/export format, not
the intended source of truth.

### Hardware crate boundary

`spiral-robotics-hardware` owns only facts and behavior whose invariants depend
on physical I/O: serial and camera discovery, camera capture, motor definitions
and calibration, motor-bus adapters, the fixed-rate control actor, bounded
motion, hardware-native observations, and the narrow capture sink driven by
that actor. Its source modules are private; callers use one reviewed flat Rust
API rather than depending on implementation layout.

The application owns robot-file and calibration codecs, HTTP and browser DTOs,
operator workflow and admission, episode identity and schemas, serialized
recording summaries, persistence, and policy inference. In particular,
hardware condition measurements have no persistence serialization contract;
storage projects them into its own durable representation, and serial-port
display wording belongs to the backend presenter.

The minimum connection input is one nonempty robot identifier, one nonempty
serial path and nonzero baud rate, a nonempty named motor set with unique bus
IDs, and an optional calibration that exactly matches those names, IDs, and
directions. Motion inputs contain a nonempty set of valid joint names and
finite normalized positions in `[-1, 1]`. Replay additionally requires at
least one nonempty valid pose, one stable joint set across all poses, a finite
positive sample rate, a start frame inside the trajectory, and a finite
non-negative resolved start time.

These preconditions are checked by invariant-preserving constructors before a
value can enter the control actor. The actor rechecks live facts that can change
after construction, including connection, calibration, torque ownership,
reported joint sets, and bus health. Construction uses focused constructors and
builders for optional state; dynamic steering uses `HardwareHandle` commands
and typed `JointGoals` rather than public mutable fields.

Every published `HardwareState` is one immutable atomic snapshot. Its sequence,
loop timing, robot observations, recording/replay progress, connection results,
control owner, reported limits, and retained issues come from the same control
publication. A successful command reply means the serialized actor accepted
and applied that transition; the next published state is the observable
postcondition. Boundary values implement the standard traits needed by callers
(`Debug`, `Clone`/`Copy`, equality, `Error`, `TryFrom`, `From`, `AsRef`, and
`IntoIterator` where their semantics apply).

The hardware public-API test composes a valid connection and replay exclusively
through the flat facade, asserts constructor rejection for malformed values,
and checks the caller-facing trait contract. Control tests verify state-machine,
safety, and snapshot postconditions; backend adapter tests verify projection
without moving application policy into the crate.

## Implementation ownership

- Browser/backend robot wire contract:
  [`spiral-robotics-api`](../../crates/spiral-robotics-api/)
- Motor definitions, encoder units, calibration and motor-health policy, native
  telemetry, motor buses, Feetech adapters, physical device discovery, camera
  capture, and fixed-rate control:
  [`spiral-robotics-hardware`](../../crates/spiral-robotics-hardware/)
- Hardware values do not depend on API DTOs. The backend projects native
  hardware state into the wire contract.
- Display joint vocabulary, order, labels, tolerant pose values, validated
  viewport frames, semantic picking, and robot geometry:
  [`spiral-robotics-viewport`](../../crates/spiral-robotics-viewport/)
- The frontend adapts live or recorded telemetry, selection, native motor
  conditions, accent assignment, and configured camera mounts into one
  viewport frame. Viewport internals do not depend on API DTOs or frontend
  state.
- Robot configuration codecs and capability policy:
  [`backend/robots`](../../crates/spiral-robotics-backend/src/robots/)
- SpiralDB robot storage:
  [`spiral-robotics-storage`](../../crates/spiral-robotics-storage/src/)
- Camera and video runtime orchestration:
  [`backend/robotics`](../../crates/spiral-robotics-backend/src/robotics/)
- Robot use cases:
  [`backend capability modules`](../../crates/spiral-robotics-backend/src/)
- Settings UI:
  [`frontend/settings/robots`](../../crates/spiral-robotics-frontend/src/settings/robots/)
