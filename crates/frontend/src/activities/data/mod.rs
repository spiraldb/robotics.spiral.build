//! Data: browse datasets and inspect one episode at a time.

pub(crate) mod query_result;
pub(crate) mod screen;

mod catalog_rail;
mod client;
mod dataset_overview;
mod import_dataset;
mod relations;
mod robot_discovery;
pub(crate) use client::DatasetClient;
pub(crate) use dataset_overview::DatasetOverview;
