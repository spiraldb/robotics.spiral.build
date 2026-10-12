//! Curation: a graph that starts at a dataset and ends in the data to train
//! on, beside the selected node's workspace.
//!
//! Every edge carries one table, one row per episode with the ranges it
//! keeps of it: a model node adds the columns its model answered, and a
//! Select keeps part of the table by conditions on them. `document` and
//! `state` are the shared contract every view reads and edits through,
//! `session` the open curation's saves and `client` its requests, and
//! `population` what reaches each node over the sample. `graph` is the
//! canvas and its workspace shell; `annotate` is the Break down subtasks
//! node, `score_progress` the Score progress node, `track_objects` the Track
//! objects node, `similar` the Find similar and Group similar subtasks nodes,
//! and `select` the Select node, where every cutoff is reviewed. `publish`
//! turns the nodes the operator chooses into catalog views, through a
//! backend run that first stores the model answers they read, and
//! `curated_view` lists those views, how far each run has got, and what each
//! column of them holds, for Data and Training.
//!
//! `path` compiles a path of the graph to one Spiral statement over the
//! stored answers, each node kind writing its own part, and to the columns a
//! fill stores them in; `preview` reads one model node's part one episode at
//! a time and holds what it read over the sample this session, a timeline as
//! a `measured_timeline`.
//!
//! `review` is the proportion every node is read in — an index that scrolls
//! beside a recording that holds still — and `thumbnail` the frame each of
//! its rows leads with. `threshold` is the strip every Select's threshold
//! slides on. `episode_tabs` opens the episodes an index's rows belong to as
//! tabs beside it. `primer` is what a model node says it is for until it has
//! measured something. `rerun` guards the handful of actions that spend money
//! or replace work already measured.
pub(crate) mod state;
pub(crate) mod workspace;

pub(in crate::activities) use client::FillRuns;
pub(crate) use curated_view::{CuratedFills, CuratedView, SubtaskColumn, ViewFill};

mod annotate;
mod client;
mod curated_view;
mod document;
mod episode_map;
mod episode_tabs;
mod graph;
mod leave_guard;
mod measure_run;
mod measured_timeline;
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
mod track_objects;
