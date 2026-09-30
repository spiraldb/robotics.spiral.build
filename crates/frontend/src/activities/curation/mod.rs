//! Curation: a graph that starts at a dataset and ends in the data to train
//! on, beside the selected node's workspace.
//!
//! Every edge carries a population — the parts of which episodes a node hands
//! on — and every node either measures what it receives or selects from it.
//! `document` and `state` are the shared contract every view reads and edits
//! through, `session` the open curation's saves and `client` its requests,
//! and `population` what flows along an edge. `graph` is the canvas
//! and its workspace shell; `annotate` is the Break down subtasks
//! measurement, `score_progress` the Score progress node, `similar` the Find
//! similar and Group similar subtasks nodes, and `select` the Select node and
//! the review of any node's rule. `publish` turns the outputs the operator
//! chooses into catalog views, through a backend run that first stores the
//! model answers they read, and `curated_view` lists those views, and how
//! far each run has got, for Data to inspect.
//!
//! `path` compiles a path of the graph to one Spiral statement over the
//! stored answers, each node kind writing its own step, and to the columns a
//! fill stores them in; `preview` runs those statements one episode at a
//! time and holds what they read over the sample this session.
//!
//! `review` is the proportion every output is read in — an index that
//! scrolls beside a recording that holds still — and `thumbnail` the frame
//! each of its rows leads with. `threshold` is the strip every rule's
//! threshold slides on. `episode_tabs` opens the episodes an index's
//! rows belong to as tabs beside it. `primer` is what a measurement says it
//! is for until it has measured something. `rerun` guards the handful of actions
//! that spend money or replace work already measured.
pub(crate) mod state;
pub(crate) mod workspace;

pub(crate) use curated_view::{CuratedFills, CuratedView, ViewFill};

mod annotate;
mod client;
mod curated_view;
mod document;
mod episode_map;
mod episode_tabs;
mod graph;
mod leave_guard;
mod path;
mod population;
mod preview;
mod primer;
mod rerun;
mod review;
mod score_progress;
mod select;
mod session;
mod similar;
mod threshold;
mod thumbnail;
