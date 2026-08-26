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

- Browser/backend robot wire contract:
  [`spiral-robotics-api`](../../crates/spiral-robotics-api/)
- Motor definitions, encoder units, calibration and motor-health policy, native
  telemetry, motor buses, Feetech adapters, physical device discovery, camera
  capture, and fixed-rate control:
  [`spiral-robotics-hardware`](../../crates/spiral-robotics-hardware/)
- Hardware values do not depend on API DTOs. The backend projects native
  hardware state into the wire contract.
- Display joint order, labels, pose values, and robot geometry:
  [`spiral-robotics-viewport`](../../crates/spiral-robotics-viewport/)
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
