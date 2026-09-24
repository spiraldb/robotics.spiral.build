//! Curation: a graph that starts at a dataset and ends in the data to train
//! on, beside the selected node's workspace.
//!
//! Every edge carries a population — the parts of which episodes a node hands
//! on — and every node either measures what it receives or selects from it.
//! `document` and `state` are the shared contract every view reads and edits
//! through, and `population` what flows along an edge. `graph` is the canvas
//! and its workspace shell; `annotate` is the Break down subtasks
//! measurement, `moments` Identify unproductive moments, `similar` the Find
//! similar and Group similar subtasks nodes, and `select` the Select node.
//!
//! `review` is the proportion every output is read in — an index that
//! scrolls beside a recording that holds still — `frames` the native frame
//! selection both the contact sheets and the poster stills are cut from, and
//! `poster` the stills themselves. `episode_tabs` opens the episodes an index's
//! rows belong to as tabs beside it. `rerun` guards the handful of actions
//! that spend money or replace work already measured.
pub(crate) mod state;
pub(crate) mod workspace;

mod annotate;
mod document;
mod episode_map;
mod episode_tabs;
mod frames;
mod graph;
mod moments;
mod population;
mod poster;
mod rerun;
mod review;
mod select;
mod similar;
