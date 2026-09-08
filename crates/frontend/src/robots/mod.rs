//! Robot selection, hardware readings, and command admission have independent owners.
mod commands;
mod selection;
mod telemetry;
pub(crate) use commands::HardwareCommands;
pub(crate) use selection::{RobotSelection, Selection};
pub(crate) use telemetry::HardwareTelemetry;
