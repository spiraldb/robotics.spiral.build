//! Training screen and its durable command state.

pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) mod history;
pub(crate) use client::TrainingClient;
