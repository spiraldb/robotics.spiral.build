//! Rollout: everything that drives the real arms. Studio (Hold and Release)
//! or a leader (Teleop) controls the robot; a policy cannot yet. The scene,
//! cameras, and Timeline show it; Record keeps it as a take. Setting the arms up lives
//! here too.

pub(crate) mod actions;
pub(crate) mod joints;
pub(crate) mod screen;
pub(crate) mod workbench;

mod next_take;
mod policy;
mod recording_client;
mod setup;
pub(crate) use recording_client::RecordingClient;

pub(crate) mod recording_actions;

mod playback;

pub(crate) mod session;

pub(crate) mod recording_review;

pub(crate) mod signals;
mod timeline;
mod transport;
