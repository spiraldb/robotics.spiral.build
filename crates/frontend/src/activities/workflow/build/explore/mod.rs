//! Explorations over a saved subtask table: rank rows against a query with
//! jev, and group rows into LLM-proposed categories assigned by jev. Owned
//! by the Explore stream.
//!
//! `questions`, `ranking`, and `categories` are the pure half — what is
//! asked, and what the answers mean — and carry the unit tests. `decisions`
//! sends the requests, `source` reads the document, and `search` and
//! `grouping` are the workspaces the canvas mounts. Both read their rows in
//! the shared `build::review` composition rather than each drawing a list and
//! a player of its own.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

mod categories;
mod decisions;
mod grouping;
mod questions;
mod ranking;
mod search;
mod source;

pub(super) use {grouping::GroupingWorkspace, search::SearchWorkspace};
