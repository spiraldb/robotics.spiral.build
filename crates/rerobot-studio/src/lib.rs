//! The complete rerobot Studio application, compiled by DX for the browser and server.
//!
//! Studio-specific code is split into `frontend` and server-gated `backend`
//! trees. Cross-boundary domain and wire contracts live in `rerobot-core`.

mod frontend;

pub use frontend::App;

#[cfg(feature = "server")]
pub mod backend;
