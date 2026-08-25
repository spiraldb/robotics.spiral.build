//! Studio destination and runtime preferences.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus project-owned Studio preferences.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    spiral: SpiralSettings,
    #[serde(default)]
    training: TrainingRuntimeSettings,
    video: VideoSettings,
}

impl DataSettings {
    pub fn new(
        spiral: SpiralSettings,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Self {
        Self {
            spiral,
            training,
            video,
        }
    }

    pub const fn spiral(&self) -> &SpiralSettings {
        &self.spiral
    }

    pub const fn training(&self) -> &TrainingRuntimeSettings {
        &self.training
    }

    pub const fn video(&self) -> &VideoSettings {
        &self.video
    }
    pub fn spiral_mut(&mut self) -> &mut SpiralSettings {
        &mut self.spiral
    }
    pub fn training_mut(&mut self) -> &mut TrainingRuntimeSettings {
        &mut self.training
    }
    pub fn video_mut(&mut self) -> &mut VideoSettings {
        &mut self.video
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
    #[serde(default = "SpiralSettings::default_spfs_endpoint")]
    spfs_endpoint: String,
    project_id: String,
    /// Public half of the client-credentials pair used by the backend.
    #[serde(default)]
    client_id: String,
    /// The secret is write-only and never crosses back to the browser.
    #[serde(default)]
    client_secret_configured: bool,
    namespace: String,
    #[serde(default)]
    endpoint_restart_required: bool,
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

impl SpiralSettings {
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
            ..Self::default()
        }
    }
    pub fn with_client(mut self, client_id: impl Into<String>, secret_configured: bool) -> Self {
        self.client_id = client_id.into();
        self.client_secret_configured = secret_configured;
        self
    }
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }
    pub const fn with_endpoint_restart_required(mut self, required: bool) -> Self {
        self.endpoint_restart_required = required;
        self
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

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub const fn client_secret_configured(&self) -> bool {
        self.client_secret_configured
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub const fn endpoint_restart_required(&self) -> bool {
        self.endpoint_restart_required
    }
    pub fn set_endpoint(&mut self, endpoint: impl Into<String>) {
        self.endpoint = endpoint.into();
    }
    pub fn set_spfs_endpoint(&mut self, endpoint: impl Into<String>) {
        self.spfs_endpoint = endpoint.into();
    }
    pub fn set_project_id(&mut self, project_id: impl Into<String>) {
        self.project_id = project_id.into();
    }
    pub fn set_client_id(&mut self, client_id: impl Into<String>) {
        self.client_id = client_id.into();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSettings {
    enabled: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl VideoSettings {
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }
    pub const fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

/// Spiral connection and write-only credential changes from the settings screen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralConnectionUpdate {
    #[serde(default = "SpiralConnectionUpdate::default_spiral_endpoint")]
    spiral_endpoint: String,
    #[serde(default = "SpiralConnectionUpdate::default_spiral_spfs_endpoint")]
    spiral_spfs_endpoint: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    clear_client_secret: bool,
}

impl SpiralConnectionUpdate {
    fn default_spiral_endpoint() -> String {
        DEFAULT_SPIRAL_ENDPOINT.to_owned()
    }

    fn default_spiral_spfs_endpoint() -> String {
        DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()
    }

    pub fn new(
        spiral_endpoint: impl Into<String>,
        spiral_spfs_endpoint: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            spiral_endpoint: spiral_endpoint.into(),
            spiral_spfs_endpoint: spiral_spfs_endpoint.into(),
            project_id: project_id.into(),
            client_id: String::new(),
            client_secret: String::new(),
            clear_client_secret: false,
        }
    }

    pub fn with_client_credentials(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.client_id = client_id.into();
        self.client_secret = client_secret.into();
        self
    }

    pub const fn with_clear_client_secret(mut self, clear_client_secret: bool) -> Self {
        self.clear_client_secret = clear_client_secret;
        self
    }
}

impl Default for SpiralConnectionUpdate {
    fn default() -> Self {
        Self::new(
            Self::default_spiral_endpoint(),
            Self::default_spiral_spfs_endpoint(),
            String::new(),
        )
    }
}

/// Update from the settings screen.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateDataSettingsRequest {
    /// Backend-local bootstrap pointer for the Spiral control-plane API.
    #[serde(flatten)]
    connection: SpiralConnectionUpdate,
    /// Backend-local bootstrap pointer for Spiral File System payloads.
    #[serde(default)]
    training: TrainingRuntimeSettings,
    video: VideoSettings,
}

impl UpdateDataSettingsRequest {
    pub fn new(
        connection: SpiralConnectionUpdate,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> Self {
        Self {
            connection,
            training,
            video,
        }
    }

    pub fn spiral_endpoint(&self) -> &str {
        &self.connection.spiral_endpoint
    }

    pub fn spiral_spfs_endpoint(&self) -> &str {
        &self.connection.spiral_spfs_endpoint
    }

    pub fn project_id(&self) -> &str {
        &self.connection.project_id
    }

    pub fn client_id(&self) -> &str {
        &self.connection.client_id
    }

    pub fn client_secret(&self) -> &str {
        &self.connection.client_secret
    }

    pub const fn clear_client_secret(&self) -> bool {
        self.connection.clear_client_secret
    }

    pub const fn training(&self) -> &TrainingRuntimeSettings {
        &self.training
    }

    pub const fn video(&self) -> &VideoSettings {
        &self.video
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionTestResponse {
    ok: bool,
    message: String,
}

impl ConnectionTestResponse {
    pub fn new(ok: bool, message: impl Into<String>) -> Self {
        Self {
            ok,
            message: message.into(),
        }
    }

    pub const fn ok(&self) -> bool {
        self.ok
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}
