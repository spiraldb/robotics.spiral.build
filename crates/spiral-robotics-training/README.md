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
- checkpoint tensor production.

All request and result values are owned and serializable. A local adapter can
call a native framework directly; a remote adapter can translate the same
operations to RPC or HTTP. Calls run on Studio worker threads, so a remote
implementation may use a blocking client without blocking the async control
loop. It must apply bounded transport timeouts.

The default implementation lives in the separate
`spiral-robotics-training-pytorch` leaf crate. Studio's server library accepts
an `Arc<dyn TrainingBackend>` and does not construct that implementation.
