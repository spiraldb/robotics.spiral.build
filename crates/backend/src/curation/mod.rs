//! Curations: one opaque, versioned graph document each, stored, listed, and
//! loaded by identity without reading its payload, the catalog views its
//! outputs are published as, and the preview runs its nodes start.
pub(crate) mod http;
pub(crate) mod manager;
mod preview;
