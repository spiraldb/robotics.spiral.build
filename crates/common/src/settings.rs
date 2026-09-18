//! Studio destination and runtime preferences.

use serde::{Deserialize, Serialize};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";

/// The immutable Spiral destination plus project-owned Studio preferences.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
    #[serde(default)]
    huggingface_token_configured: bool,
    table_prefix: String,
}

impl Default for SpiralSettings {
    fn default() -> Self {
        Self {
            endpoint: Self::default_endpoint(),
            spfs_endpoint: Self::default_spfs_endpoint(),
            project_id: String::new(),
            client_id: String::new(),
            client_secret_configured: false,
            huggingface_token_configured: false,
            table_prefix: "studio".to_owned(),
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
            ..Self::default()
        }
    }
    pub fn with_client(mut self, client_id: impl Into<String>, secret_configured: bool) -> Self {
        self.client_id = client_id.into();
        self.client_secret_configured = secret_configured;
        self
    }
    pub const fn with_huggingface_token_configured(mut self, configured: bool) -> Self {
        self.huggingface_token_configured = configured;
        self
    }

    pub const fn huggingface_token_configured(&self) -> bool {
        self.huggingface_token_configured
    }

    pub fn with_table_prefix(mut self, table_prefix: impl Into<String>) -> Self {
        self.table_prefix = table_prefix.into();
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

    pub fn table_prefix(&self) -> &str {
        &self.table_prefix
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralConnectionUpdate {
    #[serde(default = "SpiralSettings::default_endpoint")]
    spiral_endpoint: String,
    #[serde(default = "SpiralSettings::default_spfs_endpoint")]
    spiral_spfs_endpoint: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    clear_client_secret: bool,
    /// Omitted preserves the saved token; an empty string removes it.
    #[serde(default)]
    huggingface_token: Option<String>,
}

impl SpiralConnectionUpdate {
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
            huggingface_token: None,
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

    pub fn with_huggingface_token(mut self, token: Option<String>) -> Self {
        self.huggingface_token = token;
        self
    }

    pub fn huggingface_token(&self) -> Option<&str> {
        self.huggingface_token.as_deref()
    }

    pub fn into_parts(self) -> (String, String, String, String, String, bool, Option<String>) {
        (
            self.spiral_endpoint,
            self.spiral_spfs_endpoint,
            self.project_id,
            self.client_id,
            self.client_secret,
            self.clear_client_secret,
            self.huggingface_token,
        )
    }
}

impl std::fmt::Debug for SpiralConnectionUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpiralConnectionUpdate")
            .field("spiral_endpoint", &self.spiral_endpoint)
            .field("spiral_spfs_endpoint", &self.spiral_spfs_endpoint)
            .field("project_id", &self.project_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("clear_client_secret", &self.clear_client_secret)
            .field(
                "huggingface_token",
                &self.huggingface_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for SpiralConnectionUpdate {
    fn default() -> Self {
        Self::new(
            SpiralSettings::default_endpoint(),
            SpiralSettings::default_spfs_endpoint(),
            String::new(),
        )
    }
}

mod requests;
pub use requests::{ConnectionTest, SettingsUpdate};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huggingface_settings_expose_presence_and_redact_update_debug() {
        let update = SpiralConnectionUpdate::default()
            .with_client_credentials("client", "private-client-secret")
            .with_huggingface_token(Some("hf_private".into()));
        let debug = format!("{update:?}");
        assert!(!debug.contains("hf_private"));
        assert!(!debug.contains("private-client-secret"));
        let settings = SpiralSettings::default().with_huggingface_token_configured(true);
        let json = serde_json::to_value(settings).unwrap();
        assert_eq!(json["huggingface_token_configured"], true);
        assert!(json.get("huggingface_token").is_none());
        let legacy: SpiralSettings = serde_json::from_value(serde_json::json!({
            "endpoint": DEFAULT_SPIRAL_ENDPOINT, "project_id": "project", "table_prefix": "studio"
        }))
        .unwrap();
        assert!(!legacy.huggingface_token_configured());
    }
}
