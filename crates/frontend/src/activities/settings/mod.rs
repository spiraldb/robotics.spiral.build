//! Settings screen: the workspace, the robots and training runtime it uses,
//! and the Spiral account.

mod account;
pub(crate) mod panel;
mod workspace;

mod client;
pub(crate) use client::SettingsClient;
