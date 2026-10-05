//! Shared HTTP transport, project and robot connector request contexts, and
//! polling policy. Feature clients live with their UI.

pub(crate) mod completions;
pub(crate) mod http;
pub(crate) mod project;
pub(crate) mod proxy;
pub(crate) mod queries;
pub(crate) mod robot_connector;
pub(crate) mod transport;

pub(crate) use transport::CommandError;
