//! Explorations over a saved subtask table: rank rows against a query with
//! jev, and group rows into LLM-proposed categories assigned by jev. Owned
//! by the Explore stream.
//!
//! `questions`, `ranking`, and `categories` are the pure half — what is
//! asked, and what the answers mean — and carry the unit tests. `decisions`
//! sends the requests, `source` reads the document, and `search`,
//! `grouping`, and `slice` are the workspaces the canvas mounts.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

mod categories;
mod decisions;
mod grouping;
mod questions;
mod ranking;
mod search;
mod slice;
mod source;

pub(super) use {grouping::GroupingWorkspace, search::SearchWorkspace};
