//! Explorations: one opaque, versioned document each. The backend stores,
//! lists, and loads documents by identity and never reads their payload.
pub(crate) mod http;
pub(crate) mod manager;
