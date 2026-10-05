//! Studio destination and the preferences each workspace keeps.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus what one workspace keeps: its name,
/// training runtime, and robot connector.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    spiral: SpiralSettings,
    name: String,
    training: TrainingRuntimeSettings,
    robot_connector: RobotConnectorSettings,
}

impl DataSettings {
    pub fn new(
        spiral: SpiralSettings,
        name: impl Into<String>,
        training: TrainingRuntimeSettings,
        robot_connector: RobotConnectorSettings,
    ) -> Self {
        Self {
            spiral,
            name: name.into(),
            training,
            robot_connector,
        }
    }

    pub const fn spiral(&self) -> &SpiralSettings {
        &self.spiral
    }

    /// The workspace's name; empty before a workspace is chosen.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn training(&self) -> &TrainingRuntimeSettings {
        &self.training
    }

    pub const fn robot_connector(&self) -> &RobotConnectorSettings {
        &self.robot_connector
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn training_mut(&mut self) -> &mut TrainingRuntimeSettings {
        &mut self.training
    }
}

/// HTTP and Arrow IPC endpoint selected for future training sessions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeSettings {
    endpoint: String,
}

impl Default for TrainingRuntimeSettings {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_owned(),
        }
    }
}

impl TrainingRuntimeSettings {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn set_endpoint(&mut self, endpoint: impl Into<String>) {
        self.endpoint = endpoint.into();
    }
}

/// The robot connector a workspace drives: an HTTP endpoint that runs next to
/// the arms. Every browser in the workspace reaches it at this address, so
/// `localhost` means each viewer's own machine and a network address is one
/// connector everyone shares.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorSettings {
    address: String,
}

impl Default for RobotConnectorSettings {
    fn default() -> Self {
        Self {
            address: "http://localhost:7070".to_owned(),
        }
    }
}

impl RobotConnectorSettings {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralSettings {
    /// Spiral control-plane API endpoint.
    endpoint: String,
    /// Spiral File System endpoint used for relation and binary payloads.
    spfs_endpoint: String,
    project_id: String,
}

impl Default for SpiralSettings {
    fn default() -> Self {
        Self {
            endpoint: Self::default_endpoint(),
            spfs_endpoint: Self::default_spfs_endpoint(),
            project_id: String::new(),
        }
    }
}

impl SpiralSettings {
    fn default_endpoint() -> String {
        DEFAULT_SPIRAL_ENDPOINT.to_owned()
    }

    fn default_spfs_endpoint() -> String {
        DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()
    }

    pub fn new(
        endpoint: impl Into<String>,
        spfs_endpoint: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            spfs_endpoint: spfs_endpoint.into(),
            project_id: project_id.into(),
        }
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The project these settings save, as the browser names it on requests.
    pub fn project(&self) -> crate::project::ProjectKey {
        crate::project::ProjectKey::new(&self.endpoint, &self.project_id)
    }
}

mod requests;
pub use requests::{ConnectionTest, SettingsUpdate};
