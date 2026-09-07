//! Rollout screen: checkpoint selection and follower-scoped policy operation.

pub(crate) mod screen;
pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) use client::RolloutClient;
