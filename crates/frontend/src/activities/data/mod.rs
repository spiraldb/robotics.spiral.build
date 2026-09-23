//! Data: browse datasets and inspect one episode on a shared timeline.

pub(crate) mod query_result;
pub(crate) mod screen;

mod catalog_rail;
mod client;
mod dataset_overview;
mod import_dataset;
mod plots_panel;
mod relations;
mod robot_discovery;
mod robot_stage;
mod stream_choices;
mod timeline_panel;
pub(crate) use client::DatasetClient;
