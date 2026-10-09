//! Spiral destinations and project-owned Studio preferences in SQLite.

use serde::{Deserialize, Serialize};
use spiral_robotics_common::settings::{
    DEFAULT_POLICY_RUNTIME, DEFAULT_SPIRAL_ENDPOINT, DEFAULT_SPIRAL_SPFS_ENDPOINT,
};
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

    fn policy_runtime(address: &str) -> Result<Self, String> {
        Self::parse(address, "Policy runtime")
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

/// Storage-owned persisted form of the Studio preferences document: what one
/// workspace keeps besides its name.
///
/// These field names are the durable `value_json` format for
/// the managed `studio_settings` relation. They are deliberately independent
/// of the wire crate's settings types, so a contract rename can never change
/// what an existing project's preferences mean; a test pins the serialized
/// shape and another pins the defaults to the wire defaults.
///
/// Documents written before cameras moved to the robot connector also hold a
/// `video` preference, and those written before training moved to Modal a
/// `training` one; reads ignore both. Documents written before workspaces
/// kept a robot connector read as the default one, and those written before
/// they kept a policy runtime read as the default runtime. Documents written
/// before the robot connector linked itself to Studio also hold the
/// `robot_connector.address` browsers reached it at; reads ignore it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredStudioPreferences {
    #[serde(default)]
    robot_connector: StoredRobotConnectorPreference,
}

impl StoredStudioPreferences {
    /// These preferences with the robot connector asking the policy runtime
    /// at `address` for actions, validated.
    pub fn with_policy_runtime(mut self, address: &str) -> Result<Self, String> {
        self.robot_connector.policy_runtime =
            ServiceEndpoint::policy_runtime(address)?.into_string();
        Ok(self)
    }

    /// These preferences, refused unless every endpoint in them is valid: a
    /// document from an imported snapshot is untrusted.
    pub(crate) fn validated(self) -> Result<Self, String> {
        let policy_runtime = self.robot_connector.policy_runtime.clone();
        self.with_policy_runtime(&policy_runtime)
    }

    pub const fn robot_connector(&self) -> &StoredRobotConnectorPreference {
        &self.robot_connector
    }
}

/// Persisted robot connector preference: the policy runtime the workspace's
/// robot connector asks for actions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredRobotConnectorPreference {
    #[serde(default = "StoredRobotConnectorPreference::default_policy_runtime")]
    policy_runtime: String,
}

impl Default for StoredRobotConnectorPreference {
    fn default() -> Self {
        Self {
            policy_runtime: Self::default_policy_runtime(),
        }
    }
}

impl StoredRobotConnectorPreference {
    pub fn policy_runtime(&self) -> &str {
        &self.policy_runtime
    }

    fn default_policy_runtime() -> String {
        DEFAULT_POLICY_RUNTIME.to_owned()
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
    fn the_policy_runtime_requires_an_http_or_https_endpoint() {
        let preferences = StoredStudioPreferences::default();
        assert!(
            preferences
                .clone()
                .with_policy_runtime("gpu-box:50051")
                .unwrap_err()
                .contains("http:// or https://")
        );

        let staged = preferences
            .with_policy_runtime("http://gpu-box.example:50051/")
            .expect("HTTP endpoints are accepted");
        assert_eq!(
            staged.robot_connector().policy_runtime(),
            "http://gpu-box.example:50051"
        );
    }

    /// The persisted document format is storage's, not the wire crate's: these
    /// bytes are what every existing project already holds.
    #[test]
    fn the_preferences_document_format_is_frozen() {
        let json = serde_json::to_string(&StoredStudioPreferences::default()).unwrap();
        assert_eq!(
            json,
            r#"{"robot_connector":{"policy_runtime":"http://localhost:50051"}}"#
        );
        let earlier: StoredStudioPreferences = serde_json::from_str(
            r#"{"training":{"endpoint":"http://runtime.example"},"video":{"enabled":false}}"#,
        )
        .unwrap();
        assert_eq!(
            earlier.robot_connector(),
            &StoredRobotConnectorPreference::default()
        );
        let without_runtime: StoredStudioPreferences = serde_json::from_str(
            r#"{"robot_connector":{"address":"http://lab-arms.example:7070"}}"#,
        )
        .unwrap();
        assert_eq!(
            without_runtime.robot_connector(),
            &StoredRobotConnectorPreference::default()
        );
        let addressed: StoredStudioPreferences = serde_json::from_str(
            r#"{"robot_connector":{"address":"http://lab-arms.example:7070","policy_runtime":"http://gpu-box.example:50051"}}"#,
        )
        .unwrap();
        assert_eq!(
            addressed.robot_connector().policy_runtime(),
            "http://gpu-box.example:50051"
        );
    }

    /// A fresh project and a fresh browser session must agree on defaults.
    #[test]
    fn stored_preference_defaults_match_the_wire_defaults() {
        use spiral_robotics_common::settings::RobotConnectorSettings;

        let stored = StoredRobotConnectorPreference::default();
        let wire = RobotConnectorSettings::default();
        assert_eq!(stored.policy_runtime(), wire.policy_runtime());
    }
}
