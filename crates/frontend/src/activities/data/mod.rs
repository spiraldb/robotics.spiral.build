//! Data screen: episode browsing, querying, and durable screen state.

pub(crate) mod catalog;
pub(crate) mod dataset_catalog;
pub(crate) mod datasets;
mod episode_browser;
mod episode_decoder;
mod episode_playback;
pub(crate) mod query_result;
pub(crate) mod relations;
pub(crate) mod screen;
pub(crate) mod sql_draft;
pub(crate) mod state;
mod topic_messages;
pub(crate) mod workbench;

mod client;
mod import_dataset;
mod resource_tree;
pub(crate) use client::DatasetClient;

pub(crate) mod episode_cache;
