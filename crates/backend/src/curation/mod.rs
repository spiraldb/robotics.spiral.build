//! Curations: one opaque, versioned graph document each, stored, listed, and
//! loaded by identity without reading its payload, plus the native progress
//! scorer one of its nodes runs on a single recording.
pub(crate) mod http;
pub(crate) mod manager;

mod episode_progress;
