//! Data: browse datasets and inspect one episode on a shared timeline.

pub(crate) mod episode_decoder;
pub(crate) mod query_result;
pub(crate) mod screen;

mod catalog_rail;
mod client;
mod dataset_overview;
mod episode_list;
mod episode_playback;
mod episode_streams;
mod episode_transport;
mod import_dataset;
mod message_buffer;
mod plots_panel;
mod relations;
mod robot_stage;
mod timeline_panel;
pub(crate) use client::DatasetClient;

pub(crate) use episode_transport::EpisodeTransport;

pub(crate) use episode_list::OpenedEpisode;
pub(in crate::activities) use episode_playback::EpisodeCamera;
pub(crate) use episode_streams::StreamCatalog;
