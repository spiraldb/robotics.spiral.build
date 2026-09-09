//! Data screen: episode browsing, querying, and durable screen state.

pub(crate) mod dataset_catalog;
pub(crate) mod datasets;
mod episode_browser;
pub(crate) mod episode_decoder;
mod episode_playback;
mod episode_topics;
pub(crate) mod query_result;
pub(crate) mod relations;
pub(crate) mod screen;
pub(crate) mod state;
pub(crate) mod workbench;

mod client;
mod import_dataset;
mod resource_tree;
pub(crate) use client::DatasetClient;

pub(crate) mod episode_cache;
