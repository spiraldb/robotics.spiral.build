# Data

## Outcome

Data lets the user organize, inspect, query, replay, import, export, and delete
the durable results of recording and training.

## Feature map

- Episode catalog and dataset operations are defined here.
- [Timeline and replay](timeline-and-replay.md)
- [SQL query](query.md)
- [SpiralDB persistence and schema](persistence.md)

## User flow

1. The user opens Data and selects a dataset.
2. Studio lists its episodes in episode order.
3. The user selects an episode to inspect or replay.
4. The user may switch to Query without changing the route layout.
5. The user may import a supported dataset or export the selected dataset.

## Datasets and episodes

- **DATA-001** One dataset must contain many ordered episodes.
- **DATA-002** The episode list must remain ordered by episode, not grouped by
  persistence or publication state.
- **DATA-003** Each episode row must offer selection, replay, and an icon-only
  delete action without requiring selection first.
- **DATA-004** A publication failure must be visible on the episode and offer
  Retry.
- **DATA-005** Episodes and Query must be top-level panels in the Data route.
- **DATA-006** Episode import and export belong to the Episodes panel.
- **DATA-007** Deleting an episode must delete the episode and its owned streams
  as one operation.

## Import and export

- **DATA-008** Import must accept supported LeRobot dataset packages.
- **DATA-009** Import must validate the complete candidate before writing
  durable rows.
- **DATA-010** A failed import must not leave partial rows or a quarantine area.
- **DATA-011** Export should produce one LeRobot-compatible package.
- **DATA-012** Import and export must preserve the episode semantics needed for
  inspection, training, and replay.

## Feature relationships

- [Recording](../recording/README.md) creates episodes.
- [Training](../training/README.md) consumes a dataset and creates runs and
  checkpoints.
- [Rollout](../rollout/README.md) consumes a checkpoint and records rollout
  evidence.

## Implementation ownership

- Data screen:
  [`frontend/screens/data`](../../crates/spiral-robotics-studio/src/frontend/screens/data/)
- Episode import, export, and current file adapter:
  [`backend/recording`](../../crates/spiral-robotics-studio-backend/src/backend/recording/)
- Dataset and episode use cases:
  [`backend/application`](../../crates/spiral-robotics-studio-backend/src/backend/application/)
- Stable relation names:
  [`spiral-robotics-contracts/src/spiral.rs`](../../crates/spiral-robotics-contracts/src/spiral.rs)

