//! The complete rerobot Studio application, compiled by DX for the browser and server.

mod activity;
mod api;
mod app;
mod commands;
mod control;
mod data_workbench;
mod icons;
mod live_workbench;
mod modes;
mod recording_signals;
mod settings_panel;
mod setup;
mod state;
mod studio_timeline;
mod training;
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
pub mod training_manager;
#[cfg(feature = "server")]
pub mod video_capture;
#[cfg(feature = "server")]
pub mod workspace;
