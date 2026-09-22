//! Explore: a dataset → measurement → exploration graph beside the selected
//! node's workspace, next to the Agent view of the same Workflow activity.
//!
//! `document` and `state` are the shared contract every view reads and edits
//! through. `graph` is the canvas and workspace shell, `annotate`
//! the Break down subtasks measurement, and `similar` the Find similar and
//! Group similar subtasks explorations.
//!
//! `review` is the proportion all three outputs are read in — an index that
//! scrolls beside a recording that holds still — `frames` the native frame
//! selection both the contact sheets and the poster stills are cut from, and
//! `poster` the stills themselves. `rerun` guards the handful of actions that
//! spend money or replace work already measured.
pub(super) mod annotate;
pub(super) mod document;
mod frames;
pub(super) mod graph;
mod poster;
mod rerun;
mod review;
pub(super) mod similar;
pub(super) mod state;
