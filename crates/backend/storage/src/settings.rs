//! Backend connection settings and project-owned Studio preferences in SQLite.

use serde::{Deserialize, Serialize};

use spiral_robotics_common::settings::{DEFAULT_SPIRAL_ENDPOINT, DEFAULT_SPIRAL_SPFS_ENDPOINT};
use url::Url;

use crate::ProviderKeys;

/// Project-owned Studio preference persistence.
pub struct StudioPreferencePersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> StudioPreferencePersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    pub async fn load(&self) -> Result<StoredStudioPreferences, String> {
        use crate::studio::studio_settings;
        use sea_orm::EntityTrait;
        let row = studio_settings::Entity::find_by_id(self.project_store.key().to_owned())
            .one(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        row.map(|row| serde_json::from_value(row.document).map_err(|error| error.to_string()))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub async fn save(&self, preferences: &StoredStudioPreferences) -> Result<(), String> {
        use crate::studio::studio_settings;
        use sea_orm::{EntityTrait, Set};
        let row = studio_settings::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            document: Set(serde_json::to_value(preferences).map_err(|error| error.to_string())?),
        };
        studio_settings::Entity::insert(row)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(studio_settings::Column::Project)
                    .update_column(studio_settings::Column::Document)
                    .to_owned(),
            )
            .exec(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// The connection Studio saved last, read once at startup.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SavedConnection {
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    spiral_endpoint: String,
    #[serde(default)]
    spiral_spfs_endpoint: String,
}

/// The backend settings document: the saved connection and the provider keys,
/// side by side in one SQLite row that project bundles never contain.
#[derive(Default, Deserialize, Serialize)]
struct BackendSettingsDocument {
    #[serde(flatten)]
    connection: SavedConnection,
    #[serde(flatten)]
    keys: ProviderKeys,
}

impl SavedConnection {
    pub fn project_id(&self) -> Option<String> {
        Self::non_empty(&self.project_id)
    }

    pub fn spiral_endpoint(&self) -> Option<String> {
        Self::non_empty(&self.spiral_endpoint)
    }

    pub fn spiral_spfs_endpoint(&self) -> Option<String> {
        Self::non_empty(&self.spiral_spfs_endpoint)
    }

    fn non_empty(value: &str) -> Option<String> {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }
}

/// Validated bootstrap destination for the one Spiral project connection.
///
/// Construction keeps endpoints and project identity together so callers
/// cannot partially reconfigure a live store. The signed-in user's access is
/// not part of the destination: every project reads it from [`crate::SpiralAccess`].
#[derive(Clone, Eq, PartialEq)]
pub struct SpiralConnectionSettings {
    endpoint: String,
    spfs_endpoint: String,
    project_id: Option<String>,
}

impl SpiralConnectionSettings {
    pub fn new(endpoint: String, spfs_endpoint: String) -> Result<Self, String> {
        Ok(Self {
            endpoint: ServiceEndpoint::spiral(&endpoint)?.into_string(),
            spfs_endpoint: ServiceEndpoint::spiral_spfs(&spfs_endpoint)?.into_string(),
            project_id: None,
        })
    }

    pub fn with_project(mut self, project_id: &str) -> Result<Self, String> {
        self.project_id = Some(Self::validated_project_id(project_id)?);
        Ok(self)
    }

    /// The same endpoints before a project is chosen.
    pub fn without_project(mut self) -> Self {
        self.project_id = None;
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn spfs_endpoint(&self) -> &str {
        &self.spfs_endpoint
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    fn validated_project_id(project_id: &str) -> Result<String, String> {
        let project_id = project_id.trim();
        if project_id.is_empty()
            || !project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("Spiral project must contain only letters, numbers, '_' or '-'".to_owned());
        }
        Ok(project_id.to_owned())
    }
}

/// A validated HTTP service destination used by Spiral Robotics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceEndpoint(String);

impl ServiceEndpoint {
    pub fn spiral(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Spiral API endpoint")
    }

    pub fn spiral_spfs(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Spiral SpFS endpoint")
    }

    fn training_runtime(endpoint: &str) -> Result<Self, String> {
        Self::parse(endpoint, "Training runtime endpoint")
    }

    pub fn environment_spiral() -> Self {
        Self(
            std::env::var("SPIRAL__SERVER__URL")
                .unwrap_or_else(|_| DEFAULT_SPIRAL_ENDPOINT.to_owned()),
        )
    }

    pub fn environment_spiral_spfs() -> Self {
        Self(
            std::env::var("SPIRAL__SPFS__URL")
                .unwrap_or_else(|_| DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()),
        )
    }

    pub fn into_string(self) -> String {
        self.0
    }

    fn parse(endpoint: &str, label: &str) -> Result<Self, String> {
        let endpoint = endpoint.trim();
        let parsed = Url::parse(endpoint)
            .map_err(|error| format!("{label} must be an http:// or https:// URL: {error}"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(format!(
                "{label} must be an http:// or https:// URL with a host"
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(format!("{label} must not contain embedded credentials"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(format!(
                "{label} must not contain a query string or fragment"
            ));
        }
        Ok(Self(parsed.as_str().trim_end_matches('/').to_owned()))
    }
}

/// Private connection settings persisted alongside project-owned Studio records.
pub struct BackendStateStore {
    studio: crate::StudioStore,
}

impl BackendStateStore {
    pub fn open_default() -> Result<Self, String> {
        Ok(Self::new(crate::StudioStore::open_default()?))
    }

    pub fn new(studio: crate::StudioStore) -> Self {
        Self { studio }
    }

    pub fn studio(&self) -> &crate::StudioStore {
        &self.studio
    }

    /// Open the database and read the saved connection and provider keys.
    pub async fn initialize(&self) -> Result<(SavedConnection, ProviderKeys), String> {
        self.studio.initialize().await?;
        let saved: BackendSettingsDocument = match self.studio.backend_settings().await? {
            Some(document) => {
                serde_json::from_value(document).map_err(|error| error.to_string())?
            }
            None => BackendSettingsDocument::default(),
        };
        Ok((saved.connection, saved.keys))
    }

    /// Save the connection a browser starts on; without a project, the next
    /// browser is asked to choose one. The saved keys stay as they are.
    pub async fn save_connection(&self, settings: &SpiralConnectionSettings) -> Result<(), String> {
        let next = SavedConnection {
            project_id: settings.project_id().unwrap_or_default().to_owned(),
            spiral_endpoint: settings.endpoint().to_owned(),
            spiral_spfs_endpoint: settings.spfs_endpoint().to_owned(),
        };
        self.studio
            .patch_backend_settings(serde_json::to_value(&next).map_err(|error| error.to_string())?)
            .await
    }

    /// Save the provider keys. The saved connection stays as it is.
    pub(crate) async fn save_keys(&self, keys: &ProviderKeys) -> Result<(), String> {
        self.studio.patch_backend_settings(keys.merge_patch()).await
    }
}

/// Storage-owned persisted form of the Studio preferences document.
///
/// These field names are the durable `value_json` format for
/// the managed `studio_settings` relation. They are deliberately independent
/// of the wire crate's settings types, so a contract rename can never change
/// what an existing project's preferences mean; a test pins the serialized
/// shape and another pins the defaults to the wire defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredStudioPreferences {
    #[serde(default)]
    training: StoredTrainingRuntimePreference,
    #[serde(default)]
    video: StoredVideoPreference,
}

impl StoredStudioPreferences {
    /// Validate raw preference edits into the persisted form.
    pub fn from_settings(
        training_endpoint: &str,
        video_enabled: bool,
    ) -> Result<StoredStudioPreferences, String> {
        let endpoint = ServiceEndpoint::training_runtime(training_endpoint.trim())?.into_string();
        Ok(StoredStudioPreferences::new(
            StoredTrainingRuntimePreference { endpoint },
            StoredVideoPreference {
                enabled: video_enabled,
            },
        ))
    }

    const fn new(training: StoredTrainingRuntimePreference, video: StoredVideoPreference) -> Self {
        Self { training, video }
    }

    pub const fn training(&self) -> &StoredTrainingRuntimePreference {
        &self.training
    }
    pub const fn video(&self) -> &StoredVideoPreference {
        &self.video
    }
}

/// Persisted training-runtime preference: the endpoint future sessions dial.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredTrainingRuntimePreference {
    endpoint: String,
}

impl Default for StoredTrainingRuntimePreference {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_owned(),
        }
    }
}

impl StoredTrainingRuntimePreference {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Persisted camera-capture preference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredVideoPreference {
    enabled: bool,
}

impl Default for StoredVideoPreference {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl StoredVideoPreference {
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backend_state_round_trips_only_the_spiral_bootstrap_pointers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("studio.sqlite3");
        let store = BackendStateStore::new(crate::StudioStore::at(path.clone()));
        let (empty, no_keys) = store.initialize().await.unwrap();
        assert_eq!(empty.project_id(), None);
        assert_eq!(empty.spiral_endpoint(), None);
        assert_eq!(empty.spiral_spfs_endpoint(), None);
        assert_eq!(no_keys, ProviderKeys::default());

        let settings = SpiralConnectionSettings::new(
            " http://127.0.0.1:4279/ ".to_owned(),
            " http://127.0.0.1:4295/ ".to_owned(),
        )
        .unwrap()
        .with_project(" project-123 ")
        .unwrap();
        let keys = ProviderKeys::default().updated(
            &spiral_robotics_common::providers::ProviderKeysUpdate::default()
                .with_llm_api_key(Some("sk-or-private".into()))
                .with_huggingface_token(Some("hf_private".into())),
        );
        store.save_keys(&keys).await.unwrap();
        // Saving the connection keeps the keys, and saving keys keeps the connection.
        store.save_connection(&settings).await.unwrap();
        let reopen = || BackendStateStore::new(crate::StudioStore::at(path.clone()));
        let (saved, saved_keys) = reopen().initialize().await.unwrap();
        assert_eq!(saved.project_id().as_deref(), Some("project-123"));
        assert_eq!(
            saved.spiral_endpoint().as_deref(),
            Some("http://127.0.0.1:4279")
        );
        assert_eq!(
            saved.spiral_spfs_endpoint().as_deref(),
            Some("http://127.0.0.1:4295")
        );
        assert_eq!(saved_keys, keys);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(store.studio().path().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let without_token = keys.updated(
            &spiral_robotics_common::providers::ProviderKeysUpdate::default()
                .with_huggingface_token(Some(String::new())),
        );
        store.save_keys(&without_token).await.unwrap();
        let (saved, saved_keys) = reopen().initialize().await.unwrap();
        assert_eq!(saved.project_id().as_deref(), Some("project-123"));
        assert_eq!(saved_keys.huggingface_token(), None);
        assert_eq!(saved_keys.llm_api_key(), Some("sk-or-private"));
    }

    #[test]
    fn spiral_endpoint_rejects_non_http_and_embedded_credentials() {
        assert!(ServiceEndpoint::spiral("file:///tmp/spiral").is_err());
        assert!(ServiceEndpoint::spiral("https://user:secret@example.com").is_err());
        assert!(ServiceEndpoint::spiral("http://127.0.0.1:4279?debug=1").is_err());
        assert!(ServiceEndpoint::spiral_spfs("file:///tmp/spfs").is_err());
        assert!(ServiceEndpoint::spiral_spfs("https://user:secret@example.com").is_err());
    }

    #[test]
    fn training_runtime_requires_an_http_or_https_endpoint() {
        assert!(
            StoredStudioPreferences::from_settings("ftp://runtime.example", true)
                .unwrap_err()
                .contains("http:// or https://")
        );

        let staged =
            StoredStudioPreferences::from_settings(" http://runtime.example:50051 ", false)
                .expect("HTTP runtime endpoint is accepted");
        assert_eq!(staged.training().endpoint(), "http://runtime.example:50051");
        assert!(!staged.video().enabled());
    }

    /// The persisted document format is storage's, not the wire crate's: these
    /// bytes are what every existing project already holds.
    #[test]
    fn the_preferences_document_format_is_frozen() {
        let json = serde_json::to_string(&StoredStudioPreferences::default()).unwrap();
        assert_eq!(
            json,
            r#"{"training":{"endpoint":"http://127.0.0.1:50051"},"video":{"enabled":true}}"#
        );
    }

    /// A fresh project and a fresh browser session must agree on defaults.
    #[test]
    fn stored_preference_defaults_match_the_wire_defaults() {
        use spiral_robotics_common::settings::{TrainingRuntimeSettings, VideoSettings};

        assert_eq!(
            StoredTrainingRuntimePreference::default().endpoint(),
            TrainingRuntimeSettings::default().endpoint()
        );
        assert_eq!(
            StoredVideoPreference::default().enabled(),
            VideoSettings::default().enabled()
        );
    }
}
