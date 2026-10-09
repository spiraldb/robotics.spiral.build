//! Studio destination and the preferences each workspace keeps.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus what one workspace keeps: its name
/// and robot connector.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    spiral: SpiralSettings,
    name: String,
    robot_connector: RobotConnectorSettings,
}

impl DataSettings {
    pub fn new(
        spiral: SpiralSettings,
        name: impl Into<String>,
        robot_connector: RobotConnectorSettings,
    ) -> Self {
        Self {
            spiral,
            name: name.into(),
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

    pub const fn robot_connector(&self) -> &RobotConnectorSettings {
        &self.robot_connector
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}

/// The robot connector a workspace drives. It runs next to the arms and links
/// itself to Studio with the token the workspace issued, so Studio needs no
/// address for it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorSettings {
    /// Whether the workspace has issued a token a connector can link with.
    token_issued: bool,
}

impl RobotConnectorSettings {
    pub fn new(token_issued: bool) -> Self {
        Self { token_issued }
    }

    pub const fn token_issued(&self) -> bool {
        self.token_issued
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
