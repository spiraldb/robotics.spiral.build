//! Curation: judge a dataset's episodes with plain rules, see which stayed
//! and which left and why, and record the transforms the kept ones receive.

pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) use client::CurationClient;
mod interpretation;

mod document_bar;
mod episode;
mod episode_review;
mod pipeline;
mod progress;
mod recipe;
mod rule_composer;
mod sheet;
