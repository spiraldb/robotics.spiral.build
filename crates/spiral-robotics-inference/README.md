# Spiral Robotics Inference Contract

Framework-neutral policy types, model artifacts, and loader interfaces shared
by Studio's HTTP client and remote compute runtimes. This crate contains no
PyTorch, LibTorch, device discovery, or native tensor backend.

The implementation lives in the top-level Python service under `training/`.
Keeping it out of the Rust workspace prevents Cargo feature unification from
linking tensor libraries into the Studio server.
