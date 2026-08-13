//! The complete Spiral Robotics Studio application, compiled by DX for the browser and server.
//!
//! Studio-specific code is split into `frontend` and server-gated `backend`
//! trees. Domain policy lives in `spiral-robotics-core`; delivery and persistence
//! representations live in `spiral-robotics-contracts`.

mod frontend;

pub use frontend::App;

#[cfg(feature = "server")]
pub mod backend;
