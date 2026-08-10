//! The complete rerobot Studio application, compiled by DX for the browser and server.

mod api;
mod app;
mod cameras;
mod control;
mod icons;
mod modes;
mod setup;
mod state;
mod timeline;
mod viewport;

pub use app::App;

#[cfg(feature = "server")]
pub mod camera_manager;
#[cfg(feature = "server")]
pub mod dataset;
#[cfg(feature = "server")]
pub mod episodes;
#[cfg(feature = "server")]
pub mod hardware;
#[cfg(feature = "server")]
pub mod routes;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod workspace;
