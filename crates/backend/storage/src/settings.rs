//! Spiral destinations and project-owned Studio preferences in SQLite.

use serde::{Deserialize, Serialize};
use spiral_robotics_common::settings::{DEFAULT_SPIRAL_ENDPOINT, DEFAULT_SPIRAL_SPFS_ENDPOINT};
use url::Url;

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

/// Validated destination of one Spiral project connection.
///
/// Construction keeps endpoints and project identity together so callers
/// cannot partially reconfigure a live store. The signed-in user's access is
/// not part of the destination: each project reads it from its session's
/// [`crate::SpiralAccess`].
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

/// Storage-owned persisted form of the Studio preferences document.
///
/// These field names are the durable `value_json` format for
/// the managed `studio_settings` relation. They are deliberately independent
/// of the wire crate's settings types, so a contract rename can never change
/// what an existing project's preferences mean; a test pins the serialized
/// shape and another pins the defaults to the wire defaults.
///
/// Documents written before cameras moved to the robot connector also hold a
/// `video` preference; reads ignore it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredStudioPreferences {
    #[serde(default)]
    training: StoredTrainingRuntimePreference,
}

impl StoredStudioPreferences {
    /// Validate raw preference edits into the persisted form.
    pub fn from_settings(training_endpoint: &str) -> Result<StoredStudioPreferences, String> {
        let endpoint = ServiceEndpoint::training_runtime(training_endpoint.trim())?.into_string();
        Ok(StoredStudioPreferences {
            training: StoredTrainingRuntimePreference { endpoint },
        })
    }

    pub const fn training(&self) -> &StoredTrainingRuntimePreference {
        &self.training
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

#[cfg(test)]
mod tests {
    use super::*;

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
            StoredStudioPreferences::from_settings("ftp://runtime.example")
                .unwrap_err()
                .contains("http:// or https://")
        );

        let staged = StoredStudioPreferences::from_settings(" http://runtime.example:50051 ")
            .expect("HTTP runtime endpoint is accepted");
        assert_eq!(staged.training().endpoint(), "http://runtime.example:50051");
    }

    /// The persisted document format is storage's, not the wire crate's: these
    /// bytes are what every existing project already holds.
    #[test]
    fn the_preferences_document_format_is_frozen() {
        let json = serde_json::to_string(&StoredStudioPreferences::default()).unwrap();
        assert_eq!(
            json,
            r#"{"training":{"endpoint":"http://127.0.0.1:50051"}}"#
        );
        let earlier: StoredStudioPreferences = serde_json::from_str(
            r#"{"training":{"endpoint":"http://runtime.example"},"video":{"enabled":false}}"#,
        )
        .unwrap();
        assert_eq!(earlier.training().endpoint(), "http://runtime.example");
    }

    /// A fresh project and a fresh browser session must agree on defaults.
    #[test]
    fn stored_preference_defaults_match_the_wire_defaults() {
        use spiral_robotics_common::settings::TrainingRuntimeSettings;

        assert_eq!(
            StoredTrainingRuntimePreference::default().endpoint(),
            TrainingRuntimeSettings::default().endpoint()
        );
    }
}
