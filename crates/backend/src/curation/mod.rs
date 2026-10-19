//! Curations: one opaque, versioned graph document each, stored, listed, and
//! loaded by identity without reading its payload; the fill runs that store
//! the model answers its outputs read and publish them as catalog views,
//! asking the Modal services for progress scores and object tracks, and
//! OpenRouter's models for subtasks and their judgments.
mod break_down;
mod fill;
pub(crate) mod http;
pub(crate) mod manager;
mod object_tracking;
mod openrouter;
mod progress_service;
mod subtask_judgment;
mod video_service;
