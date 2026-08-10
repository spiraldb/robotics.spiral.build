//! The complete rerobot Studio application, compiled by DX for the browser and server.

mod api;
mod app;
mod cameras;
mod control;
mod data_panel;
mod data_workbench;
mod icons;
mod live_workbench;
mod modes;
mod setup;
// The signal store is filled by the browser's event stream and drawn by the
// browser's renderer, so the server build compiles it without ever calling it.
// It still has to compile there: the server renders the first tree.
#[cfg_attr(not(feature = "web"), allow(dead_code))]
mod signals;
mod state;
mod studio_timeline;
mod timeline;
mod viewport;

pub use app::App;

#[cfg(feature = "server")]
pub mod camera_manager;
#[cfg(feature = "server")]
pub mod data_settings;
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
pub mod spiral_episode;
#[cfg(feature = "server")]
pub mod storage_manager;
#[cfg(feature = "server")]
pub mod video_capture;
#[cfg(feature = "server")]
pub mod workspace;
