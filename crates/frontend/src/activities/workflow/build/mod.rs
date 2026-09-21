//! Build: a dataset → measurement → exploration graph beside the selected
//! node's workspace, next to the Agent view of the same Workflow activity.
//!
//! Ownership while the streams run in parallel (remove this note on merge):
//! `document`, `client`, and `state` are the shared contract; extend them,
//! never change existing signatures. `graph` is the canvas and workspace shell.
//! `annotate` is the Break down subtasks measurement. `explore` is Find
//! similar subtasks and Group similar subtasks.
pub(super) mod annotate;
pub(super) mod client;
pub(super) mod document;
pub(super) mod explore;
pub(super) mod graph;
pub(super) mod state;
