//! What the robot connector says about itself.

use serde::{Deserialize, Serialize};

/// A reachable robot connector, the build it runs, and whether it can
/// simulate arms (a microcontroller cannot draw their cameras).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorHealth {
    version: String,
    simulates_arms: bool,
}

impl RobotConnectorHealth {
    pub fn new(version: impl Into<String>, simulates_arms: bool) -> Self {
        Self {
            version: version.into(),
            simulates_arms,
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub const fn simulates_arms(&self) -> bool {
        self.simulates_arms
    }
}
