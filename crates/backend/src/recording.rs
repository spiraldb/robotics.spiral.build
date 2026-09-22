//! Recording takes, replaying them, and the episodes they produce.

pub(crate) mod commands;
pub(crate) mod http;
pub(crate) mod session;
pub(crate) mod take;

mod catalog;
mod episode_arms;
mod queries;
mod replay;
mod seek;
