//! Training runs (`docs/training.md`): Studio admits a run, uploads the data
//! it trains on to the Modal training service, and reports where each run is.

mod admission;
pub(crate) mod http;
mod input;
pub(crate) mod manager;
mod service;
mod upload;
