//! Studio destination and runtime preferences.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus project-owned Studio preferences.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    pub spiral: SpiralSettings,
    #[serde(default)]
    pub training: TrainingRuntimeSettings,
    pub video: VideoSettings,
}

/// HTTP and Arrow IPC endpoint selected for future training sessions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeSettings {
    pub endpoint: String,
}

impl Default for TrainingRuntimeSettings {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralSettings {
    /// Spiral control-plane API endpoint.
    pub endpoint: String,
    /// Spiral File System endpoint used for relation and binary payloads.
    #[serde(default = "default_spiral_spfs_endpoint")]
    pub spfs_endpoint: String,
    pub project_id: String,
    /// Public half of the client-credentials pair used by the backend.
    #[serde(default)]
    pub client_id: String,
    /// The secret is write-only and never crosses back to the browser.
    #[serde(default)]
    pub client_secret_configured: bool,
    pub namespace: String,
    #[serde(default)]
    pub endpoint_restart_required: bool,
}

impl Default for SpiralSettings {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_SPIRAL_ENDPOINT.to_owned(),
            spfs_endpoint: DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned(),
            project_id: String::new(),
            client_id: String::new(),
            client_secret_configured: false,
            namespace: "spiral_robotics".to_owned(),
            endpoint_restart_required: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSettings {
    pub enabled: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Update from the settings screen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateDataSettingsRequest {
    /// Backend-local bootstrap pointer for the Spiral control-plane API.
    #[serde(default = "default_spiral_endpoint")]
    pub spiral_endpoint: String,
    /// Backend-local bootstrap pointer for Spiral File System payloads.
    #[serde(default = "default_spiral_spfs_endpoint")]
    pub spiral_spfs_endpoint: String,
    /// Backend-local bootstrap pointer used to select the one active Spiral
    /// project before project-owned settings can be read.
    #[serde(default)]
    pub project_id: String,
    /// OAuth client ID passed to SpiralDB as the ADBC database username.
    #[serde(default)]
    pub client_id: String,
    /// Write-only client secret. An empty value preserves the saved secret.
    #[serde(default)]
    pub client_secret: String,
    /// Explicitly remove a previously saved client secret.
    #[serde(default)]
    pub clear_client_secret: bool,
    #[serde(default)]
    pub training: TrainingRuntimeSettings,
    pub video: VideoSettings,
}

impl Default for UpdateDataSettingsRequest {
    fn default() -> Self {
        Self {
            spiral_endpoint: default_spiral_endpoint(),
            spiral_spfs_endpoint: default_spiral_spfs_endpoint(),
            project_id: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            clear_client_secret: false,
            training: TrainingRuntimeSettings::default(),
            video: VideoSettings::default(),
        }
    }
}

fn default_spiral_endpoint() -> String {
    DEFAULT_SPIRAL_ENDPOINT.to_owned()
}

fn default_spiral_spfs_endpoint() -> String {
    DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionTestResponse {
    pub ok: bool,
    pub message: String,
}
