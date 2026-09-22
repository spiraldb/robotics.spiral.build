//! Build: one opaque, versioned document per Build graph. The backend stores,
//! lists, and loads documents by identity and never reads their payload.
pub(crate) mod commands;
pub(crate) mod http;
pub(crate) mod manager;
