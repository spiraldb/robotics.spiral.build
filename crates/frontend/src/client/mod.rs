//! Shared HTTP transport, project lifetime, and polling policy. Feature clients live with their UI.

pub(crate) mod http;
pub(crate) mod project;
pub(crate) mod queries;
pub(crate) mod transport;

pub(crate) use transport::CommandError;
