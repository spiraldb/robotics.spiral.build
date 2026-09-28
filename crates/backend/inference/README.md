# Spiral Robotics Inference Contract

Framework-neutral policy types, model artifacts, and loader traits for
Studio's compute-runtime client. This crate contains no PyTorch, LibTorch,
device discovery, or native tensor backend.

The runtime is the Python service under `training/`. Keeping it out of the
Rust workspace prevents Cargo feature unification from linking tensor
libraries into the Studio server.
