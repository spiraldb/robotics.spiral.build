//! Curation: a graph that starts at a dataset and ends in the data to train
//! on, beside the selected node's workspace.
//!
//! Every edge carries a population — the parts of which episodes a node hands
//! on — and every node either measures what it receives or selects from it.
//! `document` and `state` are the shared contract every view reads and edits
//! through, and `population` what flows along an edge. `graph` is the canvas
//! and its workspace shell; `annotate` is the Break down subtasks
//! measurement, `score_progress` the Score progress node, `similar` the Find
//! similar and Group similar subtasks nodes, and `select` the Select node.
//!
//! `path` compiles a path of the graph to one Spiral statement, each node
//! kind writing its own step; `preview` runs those statements one episode at
//! a time and holds what they read over the sample this session.
//!
//! `review` is the proportion every output is read in — an index that
//! scrolls beside a recording that holds still — and `thumbnail` the frame
//! each of its rows leads with. `episode_tabs` opens the episodes an index's
//! rows belong to as tabs beside it. `rerun` guards the handful of actions
//! that spend money or replace work already measured.
pub(crate) mod state;
pub(crate) mod workspace;

mod annotate;
mod document;
mod episode_map;
mod episode_tabs;
mod graph;
mod path;
mod population;
mod preview;
mod rerun;
mod review;
mod score_progress;
mod select;
mod similar;
mod thumbnail;
