//! What the robot connector says about itself.

use serde::{Deserialize, Serialize};

/// A reachable robot connector and the build it runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorHealth {
    version: String,
}

impl RobotConnectorHealth {
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}
