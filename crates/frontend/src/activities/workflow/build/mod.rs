//! Build: a dataset → measurement → exploration graph beside the selected
//! node's workspace, next to the Agent view of the same Workflow activity.
//!
//! `document`, `client`, and `state` are the shared contract every view reads
//! and edits through. `graph` is the canvas and workspace shell, `annotate`
//! the Break down subtasks measurement, and `explore` the Find similar and
//! Group similar subtasks explorations.
pub(super) mod annotate;
pub(super) mod client;
pub(super) mod document;
pub(super) mod explore;
pub(super) mod graph;
pub(super) mod state;
