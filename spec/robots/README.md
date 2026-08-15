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

## Implementation ownership

- Pure arm, calibration, and operation policy:
  [`spiral-robotics-core`](../../crates/spiral-robotics-core/)
- Feetech bus, discovery, and robot configuration codec:
  [`src`](../../src/)
- SpiralDB robot repository:
  [`backend/robots`](../../crates/spiral-robotics-studio-backend/src/backend/robots/)
- Hardware and camera adapters:
  [`backend/robotics`](../../crates/spiral-robotics-studio-backend/src/backend/robotics/)
- Robot use cases:
  [`backend/application`](../../crates/spiral-robotics-studio-backend/src/backend/application/)
- Settings UI:
  [`frontend/screens/settings/robots`](../../crates/spiral-robotics-studio/src/frontend/screens/settings/robots/)
