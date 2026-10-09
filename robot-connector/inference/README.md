# Spiral Robotics Inference Contract

Framework-neutral policy types, model artifacts, and loader traits for the
robot connector's policy-runtime client, and the envelope a policy travels in
from Studio to the connector. This crate contains no PyTorch, LibTorch, device
discovery, or native tensor backend.

The runtime is the Python service under `training/`. Keeping it out of the
Rust workspace prevents Cargo feature unification from linking tensor
libraries into the connector or Studio.
