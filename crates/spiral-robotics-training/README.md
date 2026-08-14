# Spiral Robotics training boundary

This crate is the framework-independent seam between Studio and model
training. It intentionally contains no PyTorch, LibTorch, device, database, or
Studio implementation dependency.

Studio owns:

- Spiral query validation and batch construction;
- run admission, cancellation, history, and metrics publication;
- checkpoint manifest validation and durable publication.

A `TrainingBackend` owns:

- health and capability reporting;
- model and optimizer session creation;
- training and validation steps;
- process telemetry for the active session;
- checkpoint tensor production.

Lifecycle and result values are owned and serializable. Batch buffers may be
owned or borrowed for one synchronous step, allowing a transport decoder to
hand contiguous tensor storage to a native framework without another CPU copy.
Calls run on Studio worker threads, so a remote implementation may use a
blocking client without blocking the async control loop.

The default model implementation lives in the separate
`spiral-robotics-training-pytorch` leaf crate. The
`spiral-robotics-training-http` crate exposes any implementation through JSON
lifecycle endpoints and Arrow IPC tensor uploads. Checkpoints are raw
SafeTensors responses. The standalone
`spiral-robotics-training-runtime` binary composes the two; it owns only
in-memory optimizer sessions and never opens Studio's database.

The versioned runtime API is deliberately small:

- `GET /v1/health`
- `POST /v1/sessions` and `DELETE /v1/sessions/{id}`
- `GET /v1/sessions/{id}/telemetry`
- `POST /v1/sessions/{id}/train?step={step}`
- `POST /v1/sessions/{id}/validate`
- `POST /v1/sessions/{id}/checkpoint`

Train and validation request bodies use
`application/vnd.apache.arrow.stream`. Each tensor column is a non-null
`FixedSizeList`: `Float32` for state, image, and action values, and `UInt8`
(`0` or `1`) for action padding. Checkpoint requests remain JSON because they
carry the publication manifest and normalization statistics; successful
responses are raw SafeTensors bytes with format and extension headers.
