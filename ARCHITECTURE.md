# Spiral Robotics architecture

Spiral Robotics is organized around product actions: configure a robot,
calibrate it, operate it, record/replay a take, train a policy, and stage a
rollout. Frameworks and storage exist to deliver those actions; they do not
define them.

## Dependency direction

```text
                         inward dependencies

  Axum / Dioxus  ─┐
  JSONL / SpiralDB ├─> application use cases ─> core entities and policy
  Feetech / camera ┘             │
                                └─ owns port traits implemented by adapters

  contracts (HTTP/events/relations) ──────────> core value types
```

- `spiral-robotics-core` is the innermost crate. It contains calibration,
  episode lifecycle, operating, arm, identity, and query policy. It has no
  database, web, async runtime, hardware, contracts, renderer, or parser dependency.
- `spiral-robotics-contracts` contains delivery and persisted relation shapes.
  It may translate to and from core policy; core never imports it.
- `backend/application` contains one-action use cases and the ports they need.
  Commands and read projections are plain data and errors are transport-neutral.
  Read controllers delegate identity admission and selection (including bounded
  timeline queries) through application-owned query ports before presenters map
  results to HTTP contracts.
- backend feature modules implement adapters for robot hardware, episode
  storage, checkpoints, cameras, and training data.
- `spiral-robotics-viewport` owns robot-description parsing and all
  renderer-facing geometry and pose construction.
- `backend/http` and `frontend` are delivery mechanisms. HTTP handlers extract
  inputs, invoke a use case, and present its output/status.

## Product invariants

- A calibration belongs to one exact motor set: names, bus IDs, and directions
  must all match before connection or motion.
- A normalized observation or motion command must be finite and inside
  `[-1, 1]`. Out-of-range data is rejected before persistence, training, replay,
  or hardware handoff; visualization-only fallbacks never authorize motion.
- Every episode frame is bound to the header's exact arm/joint shape. Time is
  finite, non-negative, and monotonic. Corrupt persisted frames fail closed in
  replay, timeline, and export paths.
- Replay admission validates the entire trajectory, including the exact joint
  set and one real, finite, consistent sample clock. Seeking fails closed if a
  joint, calibration, measurement, or conversion is missing.
- The pure control-session policy owns legal modes, exclusive operation
  ownership, torque effects, rollback, and replay phases. The hardware adapter
  performs those effects and treats an unverified torque state as unsafe.
- Episode completion, judgment, and export publish atomically. Dataset import
  validates the full candidate set in isolation before it can publish any take.
- Training and rollout registries own lifecycle transitions and single-active
  admission. A training run owns an immutable checkpoint source, so later
  Settings changes cannot retarget publication or rollout lookup. The training
  execution coordinator owns readiness, query validation, identity, admission,
  launch/stop ordering, runtime-event acceptance, terminal classification, and
  failure cleanup; the manager only performs filesystem, ADBC, Burn, Tokio,
  contract-mapping, and checkpoint-publication effects.
- Training/checkpoint application models retain semantic project, namespace,
  dataset, run, and step identity only. Storage adapters derive relation names;
  contract presenters derive any table labels required by the wire format.

## Boundary tests

`spiral-robotics-core/tests/architecture.rs` prevents outward crate/framework
dependencies and asserts the core's public domain modules. Studio's architecture
test recursively scans production application policy for adapter imports and
checks every workspace crate relationship against the inward dependency graph.
Use-case tests use fake ports to verify ordering, compensation, and refusal
behavior without HTTP, hardware, files, or a database.

When adding a feature, begin with the action and invariant. Add an
application-owned port for each external consequence, implement it in an outer
adapter, and keep transport or driver types out of the use-case signature.
