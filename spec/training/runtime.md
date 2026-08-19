# Policy runtime

## Outcome

The policy runtime is a replaceable worker shared by Training and Rollout.
For training it owns the complete data/model/checkpoint loop; Studio is only
the orchestrator. SpiralDB remains the durable data store.

## Process boundary

```mermaid
flowchart LR
    Browser["Browser Studio"] -->|"HTTP"| Backend["Studio backend"]
    Backend -->|"small job-control JSON"| Runtime["Policy runtime"]
    Runtime -->|"ADBC + SPQL + Arrow"| Spiral["SpiralDB"]
    Runtime -->|"SafeTensors checkpoint rows"| Spiral
    Backend -->|"catalog, rollout artifacts"| Spiral
    Backend --> Hardware["Arms and cameras"]
```

## Ownership

- **RUNTIME-001** Studio must select and admit runs, submit immutable job
  specifications, request stop, mirror worker evidence into the UI, and admit
  motion.
- **RUNTIME-002** A training worker must own its SpiralDB connection, SPQL
  reads, sampling, normalization, model, optimizer, validation, telemetry, and
  checkpoint writes.
- **RUNTIME-003** No training state, action, image, or Arrow batch may pass
  through Studio.
- **RUNTIME-004** SpiralDB must remain the durable store. The worker may retain
  live job state in memory, but retained jobs, metrics, checkpoints, and logs
  must have explicit bounds and expose compaction gaps to callers.
- **RUNTIME-005** A Studio shutdown must detach without stopping the worker. A
  restart must fetch and reattach to the active job, replay only evidence not
  already persisted, and tolerate temporary polling disconnections.
- **RUNTIME-006** A runtime setting change must apply only to new jobs and
  policy sessions.
- **RUNTIME-007** Settings must let the user select, save, connect, and test a
  runtime endpoint.
- **RUNTIME-008** Spiral client secrets must be write-only browser input,
  remain in the backend bootstrap store, be sent only in the job creation
  request, and never appear in snapshots, logs, or telemetry.

## Training job protocol

- **RUNTIME-009** The versioned training API must expose `POST /v1/jobs`,
  `GET /v1/jobs/{id}`, `POST /v1/jobs/{id}/stop`, and
  `GET /v1/jobs/{id}/telemetry`.
- **RUNTIME-010** `POST /v1/jobs` must be idempotent when a caller-selected job
  ID and its complete specification match. Reusing an ID with a different
  specification must return a conflict. Studio must try `GET` first when
  reattaching so it does not need to resend credentials.
- **RUNTIME-011** The immutable job request must include the Spiral API and
  SpFS endpoints, project, namespace, optional client credentials, read-only
  SQL, state/action columns, the selected Spiral schema version,
  camera/decode settings, model configuration, seed, step count, validation
  split, and checkpoint cadence.
- **RUNTIME-012** Job snapshots must include phase, current step, resolved data
  contract, append-only metrics, checkpoint metadata, logs, and the next
  replay cursor, but no secret. Polls must accept metric, checkpoint, and log
  cursors and return only newer evidence. A cursor older than worker retention
  must return an explicit history-gap description.
- **RUNTIME-013** The worker must append each SafeTensors checkpoint directly
  to the configured namespace's `training_checkpoints` relation before
  publishing its checkpoint event.
- **RUNTIME-014** Worker-side SpiralDB connections must use per-database ADBC
  `username`, `password`, `spiral.api_url`, and `spiral.spfs_url` options; they
  must not mutate process-global endpoint or credential environment variables.
- **RUNTIME-015** The worker must use the `spiraldb` Python package and prefer
  SPQL video frame selection, decode, optional resize, and Arrow image tensor
  conversion. The managed MP4 relation and index are required persisted
  derivatives of every video asset. Missing, stale, or unusable video indexes
  must fail the job as storage-integrity errors; the worker must not retrieve
  raw MP4 payloads for a separate decode path. The worker must consume SPQL
  tensors through their canonical Arrow `data` and `shape` storage fields
  rather than treating the extension struct as a flat array.
- **RUNTIME-016** The protocol must support more than one runtime
  implementation and must not require gRPC, Arrow Flight, Docker, or Compose.
- **RUNTIME-031** GPU bootstrap must validate that SpiralDB's packaged video
  runtime can be loaded and provision ABI-compatible FFmpeg shared libraries
  when the host libraries do not satisfy it. Checkpoint ingestion must keep the
  Spiral namespace in the qualified table path, remain in the configured
  project, and create the checkpoint relation on its first write.
- **RUNTIME-032** One machine-readable boundary descriptor must own the current
  Spiral schema, training API major and minor revisions, and the worker's
  supported Spiral schema range. Rust constants and Python health metadata
  must derive from that descriptor. A different API major, an older runtime
  minor, or a current schema outside the advertised range must block job
  creation with a precise compatibility error. Major revisions are for
  breaking wire changes; minor revisions are additive and backward compatible.

## Rollout protocol

- **RUNTIME-017** Rollout policy load and inference may continue to use
  SafeTensors plus Arrow IPC because live observations originate in Studio's
  hardware process.
- **RUNTIME-018** Arrow IPC metadata V5 streams must use lossless Zstandard
  compression and packed HWC RGB `UInt8` image values.
- **RUNTIME-019** The runtime must move images to the selected device before
  converting to CHW `Float32` and applying ImageNet normalization.

## Current runtime

- **RUNTIME-020** The initial runtime must use LeRobot's PyTorch ACT
  implementation.
- **RUNTIME-021** The main Rust workspace must not link PyTorch or LibTorch.
- **RUNTIME-022** The Python runtime must remain outside the Rust workspace and
  use the locked `uv` environment.
- **RUNTIME-023** Missing GPU telemetry must be reported as unavailable and
  must never fail an otherwise successful step.
- **RUNTIME-024** The runtime must bind to loopback by default. SSH local
  forwarding is the standard remote path, providing encryption for the job
  request that carries Spiral credentials; a directly reachable deployment
  must require explicit remote-access opt-in and sit behind HTTPS and service
  authentication. Studio's unauthenticated control listener must enforce the
  same loopback-by-default rule.
- **RUNTIME-025** Device strings must begin with the compact runtime device
  identifier and the runtime owns availability validation.
- **RUNTIME-026** Every optimizer metric must retain worker loader, device-copy,
  and model timing. Studio-side Arrow encode and upload timings are absent for
  worker-owned training jobs because there is no training data upload.
- **RUNTIME-027** One worker batch must request all required indexed video
  frames in one SPQL statement, even when the samples span several videos, and
  must propagate any managed-video relation or index failure.
- **RUNTIME-028** Lambda bootstrap must prefer a colocated
  `libadbc_driver_spiraldb.so` over the driver inside the locked Python package,
  so a credential-capable deployment driver is not replaced during `uv sync`.
- **RUNTIME-029** Worker batch sampling must choose an episode through a
  uniformly sampled anchor and draw the batch from that episode. This preserves
  each anchor's marginal sampling probability while bounding the number of
  distinct video segments opened by one optimizer step.
- **RUNTIME-030** Successful polling must use a bounded cadence. Consecutive
  transport failures must use exponential backoff with a fixed upper bound and
  recovery must be visible in the run log.

## Implementation ownership

- Authoritative versions and bump policy:
  [`contracts`](../../contracts/)
- Versioned contracts: [`training/contracts`](../../training/contracts/)
- Studio job client: [`training/http-client`](../../training/http-client/)
- Worker job lifecycle: [`training/src/spiral_training_runtime/jobs.py`](../../training/src/spiral_training_runtime/jobs.py)
- Worker Spiral/SPQL loader: [`training/src/spiral_training_runtime/spiral.py`](../../training/src/spiral_training_runtime/spiral.py)
- Framework-neutral inference contract: [`spiral-robotics-inference`](../../crates/spiral-robotics-inference/)
