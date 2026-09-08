//! Data screen: episode browsing, querying, and durable screen state.

pub(crate) mod catalog;
pub(crate) mod dataset_catalog;
pub(crate) mod native_datasets;
mod native_episodes;
mod native_messages;
pub(crate) mod query_result;
pub(crate) mod relations;
pub(crate) mod screen;
pub(crate) mod sql_draft;
pub(crate) mod state;
pub(crate) mod workbench;

mod client;
pub(crate) use client::DatasetClient;

pub(crate) mod episode_cache;
