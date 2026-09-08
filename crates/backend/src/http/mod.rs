//! HTTP routing and server bootstrap.

pub(crate) mod routes;
pub(crate) mod server;

pub(crate) mod delivery;

pub(crate) mod contract;

#[cfg(test)]
mod contract_tests;

mod web_assets;
