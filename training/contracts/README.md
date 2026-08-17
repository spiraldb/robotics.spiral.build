# Spiral Robotics training boundary

This crate is the framework-independent seam between Studio and model
training. It intentionally contains no PyTorch, LibTorch, device, database, or
Studio implementation dependency.

Studio owns:

- run admission, stop requests, mirrored history, and UI publication;
- the immutable selection sent to a worker job;
- rollout observations and hardware admission.

A `TrainingBackend` owns:

- health and capability reporting;
- worker-owned job creation, lookup, and stop;
- direct SpiralDB reads, sampling, and checkpoint publication for jobs;
- model and optimizer session creation;
- training and validation steps;
- process telemetry for the active session;
- checkpoint tensor production.

Lifecycle and result values are owned and serializable. Batch buffers may be
owned or borrowed for one synchronous step, allowing a transport decoder to
hand contiguous tensor storage to a native framework without another CPU copy.
Calls run on Studio worker threads, so a remote implementation may use a
blocking client without blocking the async control loop.

The sibling `http-client/` crate exposes worker jobs and rollout policy
sessions. The Python service implements LeRobot ACT and opens the job-selected
SpiralDB project itself. Training tensors never pass through Studio.

The versioned runtime API is deliberately small:

- `GET /v1/health`
- `POST /v1/jobs`, `GET /v1/jobs/{id}`, and `POST /v1/jobs/{id}/stop`
- `POST /v1/sessions` and `DELETE /v1/sessions/{id}`
- `POST /v1/policies` and `DELETE /v1/policies/{id}`
- `POST /v1/policies/{id}/warm-up`
- `POST /v1/policies/{id}/infer` with an Arrow IPC observation
- `GET /v1/sessions/{id}/telemetry`
- `POST /v1/sessions/{id}/buffer?kind=training&step={step}` with an Arrow IPC batch
- `POST /v1/sessions/{id}/train?step={step}` to consume that batch
- `POST /v1/sessions/{id}/buffer?kind=validation&step={step}` with an Arrow IPC batch
- `POST /v1/sessions/{id}/validate?step={step}` to consume that batch
- `POST /v1/sessions/{id}/checkpoint`

Training and validation buffer request bodies use
`application/vnd.apache.arrow.stream`. Studio uses Arrow IPC metadata V5 with
lossless Zstandard buffer compression; this does not change the media type or
tensor schema. Each tensor column is a non-null `FixedSizeList`: `Float32` for
state and action values, packed HWC RGB `UInt8` for image values, and `UInt8`
(`0` or `1`) for action padding. The runtime transfers image bytes to the
selected device before converting them to CHW `Float32`, dividing by 255, and
applying ImageNet normalization. Checkpoint requests remain JSON because they
carry the publication manifest and normalization statistics; successful
responses are raw SafeTensors bytes with format and extension headers.

Protocol API version 4 makes the durable training unit a worker-owned job.
Legacy session endpoints retain their one-batch buffer for compatibility, but
the Studio training path uses job control only.
