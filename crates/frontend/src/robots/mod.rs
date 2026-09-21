//! Robot selection, hardware readings, and command admission have independent owners.
mod client;
mod commands;
mod selection;
mod setup;
mod state;
pub(crate) mod visualization;
pub(crate) use client::RobotClient;
pub(crate) use commands::HardwareCommands;
pub(crate) use selection::{RobotSelection, Selection};
pub(crate) use setup::RobotSetupState;
pub(crate) use state::HardwareViewState;

mod setup_snapshot;
pub(crate) use setup_snapshot::RobotSetupSnapshot;
