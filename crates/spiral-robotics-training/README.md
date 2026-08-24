# Spiral Robotics training boundary

This crate is the framework-independent job-control seam between Studio and a
training worker. It contains no PyTorch, device, database, HTTP, or Studio
implementation dependency.

Studio owns run admission, stop requests, mirrored history, UI publication,
and the immutable job selection. A `TrainingBackend` owns health reporting and
worker-job creation, cursored lookup, and stop. The worker itself owns SpiralDB
reads, sampling, model execution, telemetry, and checkpoint publication.
Training tensors never pass through Studio.

The authoritative API major/minor and compatible Spiral schema range live in
[`../../contracts/boundaries.json`](../../contracts/boundaries.json). Rust
constants are generated from that descriptor; version literals do not belong
in clients or runtimes. The deliberately small surface is:

- `GET /v1/health`
- `POST /v1/jobs`
- `GET /v1/jobs/{id}` with metric, checkpoint, and log cursors
- `POST /v1/jobs/{id}/stop` with the same cursors
- `GET /v1/jobs/{id}/telemetry`
- `POST /v1/policies` and `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`
- `POST /v1/policies/{id}/infer` with an Arrow IPC observation

Job collections are bounded worker replay windows. Every response carries the
next cursor, and reports a history gap if a caller asks for evidence older than
the retained window. Lifecycle and result values are serializable; credentials
are request-only and excluded from debug output and snapshots.

The sibling `spiral-robotics-training-http` crate implements this contract for Studio. The
Python service implements LeRobot ACT and opens the selected SpiralDB project
itself. Arrow IPC remains only on the rollout observation/inference path,
because those live observations originate in Studio's hardware process.

See the shared [boundary bump policy](../../contracts/README.md) before changing
the request, response, persisted-schema, or compatibility contract.
