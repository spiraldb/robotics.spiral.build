# Rollout

## Outcome

Rollout runs a compatible trained checkpoint on the configured follower for a
bounded duration while Studio retains motion authority.

## User flow

1. The user selects a checkpoint.
2. Studio checks the follower, cameras, calibration, and policy runtime.
3. The user chooses a bounded duration and reviews the checkpoint's control
   rate and action horizon.
4. Studio warms the policy and enables Run.
5. Run enters one exclusive policy state.
6. Stop or any safety failure ends rollout and leaves the follower in Hold.

## Readiness and UI

- **ROLLOUT-001** Run must remain disabled until a compatible checkpoint is
  selected.
- **ROLLOUT-002** Rollout must target the configured follower only.
- **ROLLOUT-003** The follower must be connected, calibrated, and compatible
  with the checkpoint's joint contract.
- **ROLLOUT-004** Rollout must provide every camera role required by the
  checkpoint.
- **ROLLOUT-005** The route must show the follower Scene, required cameras,
  duration, control rate, action horizon, readiness, and inference evidence.
- **ROLLOUT-006** A Stop button must be next to Run while rollout is active.
- **ROLLOUT-007** The user must choose a bounded rollout duration before Run.

## Policy execution

- **ROLLOUT-008** Run must enter one exclusive policy-execution state and stop
  other motion modes.
- **ROLLOUT-009** Runtime warm-up must complete before the policy can command the
  follower.
- **ROLLOUT-010** Studio must send the exact ordered observation and camera set
  declared by the checkpoint.
- **ROLLOUT-011** The runtime must return an ordered action chunk with timing
  evidence.
- **ROLLOUT-012** The checkpoint sample rate must control action timing.
- **ROLLOUT-013** For the current ACT contract, the policy may predict 100
  actions and Studio must execute at most the first 20 before observing again.
- **ROLLOUT-014** Observation and inference must run ahead of motor consumption
  to avoid preventable pauses between action chunks.
- **ROLLOUT-015** Studio must keep a bounded action reserve and must not execute
  stale actions indefinitely.
- **ROLLOUT-016** Finite policy outputs outside normalized `[-1, 1]` must be
  clamped at the policy boundary and reported as a warning.

## Stop and failure behavior

- **ROLLOUT-017** Stop must end policy execution and put the follower in Hold.
- **ROLLOUT-018** Non-finite actions, stale observations, motor faults, mode
  changes, runtime failures, or missed safety windows must stop rollout.
- **ROLLOUT-019** A browser disconnect must not leave browser-owned policy
  motion running.
- **ROLLOUT-020** A runtime setting change must not retarget an active rollout.

## Feature relationships

- Checkpoints come from [Training](../training/README.md).
- Tensor compute uses the [policy runtime](../training/runtime.md).
- Follower motion uses [Live robot control](../robots/control.md).
- Rollout history is durable through [Data persistence](../data/persistence.md).

## Implementation ownership

- Rollout policy:
  [`spiral-robotics-studio-application/src/rollout.rs`](../../crates/spiral-robotics-studio-application/src/rollout.rs)
  and
  [`spiral-robotics-studio-application/src/policy_runtime.rs`](../../crates/spiral-robotics-studio-application/src/policy_runtime.rs)
- Runtime and hardware supervision:
  [`backend/rollout`](../../crates/spiral-robotics-studio-backend/src/backend/rollout/)
- Rollout UI:
  [`frontend/screens/rollout`](../../crates/spiral-robotics-studio/src/frontend/screens/rollout/)

