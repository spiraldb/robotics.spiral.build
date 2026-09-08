//! Live screen, controls, and semantic actions.

pub(crate) mod actions;
pub(crate) mod control;
pub(crate) mod screen;
pub(crate) mod workbench;

mod client;
pub(crate) use client::RecordingClient;

pub(crate) mod recording_actions;

mod playback;
mod timeline_actions;

pub(crate) mod session;

mod shortcuts;
