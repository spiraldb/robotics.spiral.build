//! Data: browse datasets and inspect one episode at a time.

pub(crate) mod screen;

mod catalog_rail;
mod client;
mod curated_view;
mod dataset_checks;
mod dataset_overview;
mod discovery;
mod import_dataset;
mod relations;
mod robot_discovery;
pub(crate) use client::DatasetClient;
pub(in crate::activities) use dataset_checks::CheckedEpisode;
pub(crate) use dataset_checks::DatasetChecks;
pub(in crate::activities) use dataset_overview::DatasetOverview;
pub(crate) use dataset_overview::{DatasetLedger, DatasetOverviewMemory};
