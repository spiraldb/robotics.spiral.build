//! Curations: one opaque, versioned graph document each, stored, listed, and
//! loaded by identity without reading its payload; the fill runs that store
//! the model answers its outputs read and publish them as catalog views,
//! asking the Modal services for progress scores and object tracks; and the
//! preview runs its nodes start.
mod fill;
mod held_runs;
pub(crate) mod http;
pub(crate) mod manager;
mod object_tracking;
mod preview;
mod progress_service;
mod video_service;
