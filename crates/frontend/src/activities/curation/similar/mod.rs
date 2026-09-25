//! Find similar and Group similar subtasks: each compiles to one text
//! judgment of every subtask that reaches it (`judgment`), and its preview runs
//! that step over the rows its parent's preview holds, as every measurement's
//! does.
//!
//! `categories`, `coverage`, and `profile` are the pure half — what a category
//! proposal asks and how edits change the list, what the answers mean, and how
//! a category differs from the rest — and carry the unit tests. `source` reads
//! the curation, and `search` and `grouping` are the workspaces the canvas
//! mounts. Both read their rows in the shared `review` composition, and both
//! show where their answer landed on the shared `episode_map`, rather than
//! each drawing a list, a player, and a tally of its own.
mod categories;
mod coverage;
mod grouping;
mod judgment;
mod profile;
mod search;
mod source;

pub(super) use {
    grouping::{CategoryHues, GroupingWorkspace},
    search::SearchWorkspace,
};
