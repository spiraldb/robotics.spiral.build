# Training

## Outcome

Training turns a selected SpiralDB dataset into inspectable policy runs and
durable checkpoints through a worker that reads and writes SpiralDB directly.

## User flow

1. The user selects a dataset, policy, optional source checkpoint, run name, and
   training parameters.
2. Studio verifies the selected [policy runtime](runtime.md) and submits one
   immutable job specification.
3. The worker opens SpiralDB, resolves the data contract, and trains without
   sending samples through Studio.
4. Studio polls cursor-based job deltas with bounded retry backoff and shows
   learning, validation, throughput, resource, and loader evidence.
5. The worker commits checkpoints directly to SpiralDB while training
   continues.
6. The user may stop, inspect, resume from a checkpoint, or delete the run.

## Run management

- **TRAIN-001** Training must use SpiralDB data only.
- **TRAIN-002** The user must select a dataset, policy, run name, and training
  parameters before starting.
- **TRAIN-003** Run names must distinguish concurrent and historical runs.
- **TRAIN-004** New-run controls and run history must be separate panels or
  regions.
- **TRAIN-005** Selecting a run must open its results in a closable tab.
- **TRAIN-006** The user must be able to stop an active run.
- **TRAIN-007** The user must be able to delete a run and every checkpoint it
  owns.
- **TRAIN-008** The user must be able to start a compatible run from a
  checkpoint.
- **TRAIN-009** Checkpoints created during a run must appear before the run
  finishes.
- **TRAIN-010** Studio must persist mirrored run lifecycle, configuration, and
  metrics; the worker must persist checkpoint rows directly in SpiralDB.

## Training data

- **TRAIN-011** The initial policy must be standard vision-conditioned ACT
  aligned with LeRobot behavior.
- **TRAIN-012** Training must use every camera role required by the dataset and
  policy contract.
- **TRAIN-013** Sampling must be uniform over eligible camera frames across the
  training population, not uniform over episodes of different lengths.
- **TRAIN-014** Each sample must contain camera images, joint state, an action
  chunk, padding, and normalization required by the policy.
- **TRAIN-015** Camera frame selection must use camera frame indices and stream
  rates, not joint sample frequency.
- **TRAIN-016** SpiralDB must perform supported frame selection, decode, and
  Arrow tensor conversion. Every persisted video asset must have its managed
  video relation and index; a missing or unusable index is a storage-integrity
  error that must fail the job instead of invoking a second decode path.
- **TRAIN-017** When ACT requires image resizing or normalization, the transform
  must match LeRobot behavior.
- **TRAIN-018** The worker should overlap or pipeline data preparation with
  model compute without weakening cancellation or memory bounds.
- **TRAIN-019** Validation must use whole episodes excluded from training.

## Evidence

- **TRAIN-020** The selected run must show training and validation loss on the
  same step axis.
- **TRAIN-021** The selected run must show batch-aware sample throughput, data
  preparation rate, CPU use, GPU use, GPU memory, and transfer time when
  available.
- **TRAIN-022** The interface must show the actual compute device and fallback.
- **TRAIN-023** Throughput must count every sample in the batch.
- **TRAIN-024** Related charts must use linear axes and shared or equivalent
  data zoom.
- **TRAIN-025** Temporary sleep, disconnection, or missing telemetry must not
  erase earlier metric points. Worker compaction gaps must be visible rather
  than silently presented as complete history.
- **TRAIN-026** Primary UI labels must use user concepts such as dataset,
  samples, cameras, and checkpoints instead of implementation history.

## Checkpoint contract

- **TRAIN-027** A checkpoint must contain SafeTensors and explicit format
  metadata.
- **TRAIN-028** A checkpoint must declare the exact model, cameras, joints,
  normalization, sample rate, and action horizon.
- **TRAIN-029** The worker must commit checkpoints directly to SpiralDB as the run
  progresses.
- **TRAIN-030** A runtime setting change must not retarget an active run.
- **TRAIN-031** Persisted run history must identify the selected relational
  dataset row, but must not duplicate the backend-selected Spiral project,
  namespace, or checkpoint relation. Those locations are derived from the one
  active Spiral project.

## Implementation ownership

- Training feature policy:
  [`spiral-robotics-backend/src/training/service.rs`](../../crates/spiral-robotics-backend/src/training/service.rs)
  and
  [`spiral-robotics-backend/src/training/execution.rs`](../../crates/spiral-robotics-backend/src/training/execution.rs)
- Studio job supervision:
  [`backend/training/manager.rs`](../../crates/spiral-robotics-backend/src/training/manager.rs)
- SpiralDB batch preparation and checkpoint publication:
  [`training/src/spiral_training_runtime`](../../training/src/spiral_training_runtime/)
- Training UI:
  [`frontend/activities/training`](../../crates/spiral-robotics-frontend/src/activities/training/)
- Framework-neutral training protocol:
  [`spiral-robotics-training`](../../crates/spiral-robotics-training/)
