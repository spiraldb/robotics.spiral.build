# Spiral Robotics training protocol

This crate is the HTTP contract between Studio and a training worker. It
contains no PyTorch, device, database, or HTTP client dependency.

Studio owns run admission and a small record of each run it started: its ID,
configuration, start time, and worker endpoint. The worker owns everything the
run produces: SpiralDB reads, sampling, model execution, metrics, logs, run
status, and checkpoint publication to Spiral. Studio asks the worker for a
run's status and history instead of keeping a copy, and lists checkpoints from
Spiral. Training tensors never pass through Studio.

The authoritative API major/minor and compatible storage schema range live in
[`protocol/version.json`](protocol/version.json). The current relational
schema lives with its owner in
[`../storage/schema/version.json`](../storage/schema/version.json).
Rust constants are generated from those files; version literals do not belong
in clients or runtimes. The deliberately small surface is:

- `GET /v1/health`
- `POST /v1/jobs` with `{job_id, spiral, spiral_schema_version, config}`,
  where `config` is Studio's `TrainingConfig` unchanged
- `GET /v1/jobs`: every retained job's state
- `GET /v1/jobs/{id}`: the job's state with its retained metrics and logs
- `POST /v1/jobs/{id}/stop`
- `POST /v1/policies` and `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`
- `POST /v1/policies/{id}/infer` with an Arrow IPC observation

A worker keeps a bounded, whole-run sample of each job's metrics and a tail of
its logs in memory. A job a worker no longer holds reads as unavailable in
Studio; its checkpoints remain in Spiral.

The Studio backend implements this contract in its compute-runtime adapter.
The Python service implements LeRobot ACT and opens the selected SpiralDB
project itself. Arrow IPC remains only on the rollout observation/inference
path because those live observations originate in Studio's hardware process.

See the [protocol bump policy](protocol/README.md) before changing a request,
response, or compatibility rule. Persisted-schema changes follow the
[storage schema policy](../storage/schema/README.md).
