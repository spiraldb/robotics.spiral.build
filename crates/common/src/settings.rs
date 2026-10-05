//! Studio destination and runtime preferences.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus project-owned Studio preferences.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    spiral: SpiralSettings,
    training: TrainingRuntimeSettings,
}

impl DataSettings {
    pub fn new(spiral: SpiralSettings, training: TrainingRuntimeSettings) -> Self {
        Self { spiral, training }
    }

    pub const fn spiral(&self) -> &SpiralSettings {
        &self.spiral
    }

    pub const fn training(&self) -> &TrainingRuntimeSettings {
        &self.training
    }

    pub fn spiral_mut(&mut self) -> &mut SpiralSettings {
        &mut self.spiral
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

    pub fn set_project_id(&mut self, project_id: impl Into<String>) {
        self.project_id = project_id.into();
    }
}

mod requests;
pub use requests::{ConnectionTest, SettingsUpdate};
