//! The complete Spiral Robotics Studio application, compiled by DX for the browser and server.
//!
//! Studio-specific code is split into `frontend` and server-gated `backend`
//! trees. Cross-boundary domain and wire contracts live in `spiral-robotics-core`.

mod frontend;

pub use frontend::App;

#[cfg(feature = "server")]
pub mod backend;
