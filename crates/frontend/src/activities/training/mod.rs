//! Training screen and its durable command state.
//!
//! `setup` drafts the next run: `source` the published view it trains on and
//! what that view's dataset offers, `observation` how the model sees it.
//! `run_detail` shows one run, and `workspace` lays Setup, the runs, and the
//! opened runs out.

pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) mod history;
mod observation;
mod run_detail;
mod setup;
mod source;
pub(crate) use client::TrainingClient;
