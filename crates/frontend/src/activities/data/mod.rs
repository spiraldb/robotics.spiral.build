//! Data: browse datasets and episodes, inspect one episode on a shared
//! timeline, and query the project with read-only SQL.

pub(crate) mod dataset_catalog;
pub(crate) mod episode_decoder;
pub(crate) mod query_result;
pub(crate) mod screen;
pub(crate) mod state;
pub(crate) mod workbench;

mod catalog_rail;
mod client;
mod dataset_overview;
mod episode_list;
mod episode_playback;
mod episode_streams;
mod episode_transport;
mod import_dataset;
mod message_buffer;
mod messages_panel;
mod plots_panel;
mod relations;
mod sql_panel;
mod timeline_panel;
pub(crate) use client::DatasetClient;

mod comparison_episode;
pub(crate) use comparison_episode::ComparisonEpisodeInspector;
