//! Validation for project connection and runtime preferences.

use spiral_robotics_common::settings::DataSettings;
use url::Url;

use crate::OperationError;

struct HttpEndpoint(String);

impl HttpEndpoint {
    fn parse(endpoint: &str, label: &str) -> Result<Self, OperationError> {
        let parsed = Url::parse(endpoint.trim()).map_err(|error| {
            OperationError::invalid_input(format!(
                "{label} must be an http:// or https:// URL: {error}"
            ))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(OperationError::invalid_input(format!(
                "{label} must be an http:// or https:// URL with a host"
            )));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(OperationError::invalid_input(format!(
                "{label} must not contain embedded credentials"
            )));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(OperationError::invalid_input(format!(
                "{label} must not contain a query string or fragment"
            )));
        }
        Ok(Self(parsed.as_str().trim_end_matches('/').to_owned()))
    }

    fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    spiral_endpoint: String,
    spiral_spfs_endpoint: String,
    project_id: String,
    credentials: ConnectionCredentialUpdate,
    training_endpoint: String,
    video_enabled: bool,
}

#[derive(Clone, Default, Eq, PartialEq)]
struct ConnectionCredentialUpdate {
    client_id: String,
    client_secret: Option<String>,
    huggingface_token: Option<String>,
}

impl std::fmt::Debug for ConnectionCredentialUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionCredentialUpdate")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataSettingsUpdate {
    settings: DataSettings,
    session_replaced: bool,
}

impl UpdateDataSettingsCommand {
    pub(super) fn new(
        spiral_endpoint: impl Into<String>,
        project_id: impl Into<String>,
        training_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            spiral_endpoint: spiral_endpoint.into(),
            spiral_spfs_endpoint: String::new(),
            project_id: project_id.into(),
            credentials: ConnectionCredentialUpdate::default(),
            training_endpoint: training_endpoint.into(),
            video_enabled: false,
        }
    }

    pub(super) fn with_spfs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.spiral_spfs_endpoint = endpoint.into();
        self
    }

    pub(super) fn with_client(mut self, client_id: impl Into<String>) -> Self {
        self.credentials.client_id = client_id.into();
        self
    }

    pub(super) fn with_client_secret(mut self, secret: Option<String>) -> Self {
        self.credentials.client_secret = secret;
        self
    }

    pub(super) fn with_huggingface_token(mut self, token: Option<String>) -> Self {
        self.credentials.huggingface_token = token;
        self
    }

    pub(super) fn with_video_enabled(mut self, enabled: bool) -> Self {
        self.video_enabled = enabled;
        self
    }

    fn spiral_endpoint(&self) -> &str {
        &self.spiral_endpoint
    }
    fn spiral_spfs_endpoint(&self) -> &str {
        &self.spiral_spfs_endpoint
    }
    fn project_id(&self) -> &str {
        &self.project_id
    }
    fn client_id(&self) -> &str {
        &self.credentials.client_id
    }
    fn client_secret(&self) -> Option<&str> {
        self.credentials.client_secret.as_deref()
    }
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl DataSettingsUpdate {
    pub(crate) fn new(settings: DataSettings, session_replaced: bool) -> Self {
        Self {
            settings,
            session_replaced,
        }
    }

    pub(super) fn into_parts(self) -> (DataSettings, bool) {
        (self.settings, self.session_replaced)
    }
}

impl UpdateDataSettingsCommand {
    pub(crate) fn validated(mut self) -> Result<Self, OperationError> {
        self.spiral_endpoint =
            HttpEndpoint::parse(&self.spiral_endpoint, "Spiral API endpoint")?.into_string();
        self.spiral_spfs_endpoint =
            HttpEndpoint::parse(&self.spiral_spfs_endpoint, "Spiral SpFS endpoint")?.into_string();
        self.project_id = self.project_id.trim().to_owned();
        self.credentials.client_id = self.credentials.client_id.trim().to_owned();
        self.credentials.client_secret = self
            .credentials
            .client_secret
            .map(|secret| secret.trim().to_owned());
        self.training_endpoint = self.training_endpoint.trim().to_owned();

        if self.project_id.is_empty()
            || !self
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(OperationError::InvalidInput(
                "Spiral project must contain only letters, numbers, '_' or '-'".to_owned(),
            ));
        }

        if self.credentials.client_id.is_empty()
            && self
                .credentials
                .client_secret
                .as_ref()
                .is_some_and(|secret| !secret.is_empty())
        {
            return Err(OperationError::InvalidInput(
                "Spiral client secret requires a client ID".to_owned(),
            ));
        }

        self.training_endpoint =
            HttpEndpoint::parse(&self.training_endpoint, "Training runtime endpoint")?
                .into_string();

        Ok(self)
    }
}

/// A connection edit can repair access without reading or overwriting preferences.
pub(crate) struct ProjectConnectionUpdate(spiral_robotics_common::settings::SpiralConnectionUpdate);
impl From<spiral_robotics_common::settings::SpiralConnectionUpdate> for ProjectConnectionUpdate {
    fn from(update: spiral_robotics_common::settings::SpiralConnectionUpdate) -> Self {
        Self(update)
    }
}
impl ProjectConnectionUpdate {
    pub(crate) fn resolve(
        self,
        previous: &spiral_robotics_storage::SpiralConnectionSettings,
    ) -> Result<spiral_robotics_storage::SpiralConnectionSettings, OperationError> {
        let (endpoint, spfs, project, client, secret, clear, huggingface_token) =
            self.0.into_parts();
        let huggingface_token =
            huggingface_token.or_else(|| previous.huggingface_token().map(str::to_owned));
        let endpoint = HttpEndpoint::parse(&endpoint, "Spiral endpoint")?.into_string();
        let spfs = HttpEndpoint::parse(&spfs, "SpFS endpoint")?.into_string();
        let client = client.trim();
        let credentials = if clear || client.is_empty() {
            if client.is_empty() && !secret.trim().is_empty() && !clear {
                return Err(OperationError::invalid_input(
                    "Spiral client secret requires a client ID",
                ));
            }
            None
        } else if !secret.trim().is_empty() {
            Some(spiral_robotics_storage::SpiralCredentials::new(
                client.to_owned(),
                secret.trim().to_owned(),
            ))
        } else {
            Some(
                previous
                    .credentials()
                    .filter(|credentials| credentials.client_id() == client)
                    .cloned()
                    .ok_or_else(|| {
                        OperationError::invalid_input(
                            "Enter the client secret when changing the Spiral client ID",
                        )
                    })?,
            )
        };
        spiral_robotics_storage::SpiralConnectionSettings::new(endpoint, spfs)
            .and_then(|settings| settings.with_project(project.trim()))
            .map(|settings| {
                settings
                    .with_credentials(credentials)
                    .with_huggingface_token(huggingface_token)
            })
            .map_err(OperationError::InvalidInput)
    }
}

impl From<&UpdateDataSettingsCommand> for ProjectConnectionUpdate {
    fn from(command: &UpdateDataSettingsCommand) -> Self {
        Self(
            spiral_robotics_common::settings::SpiralConnectionUpdate::new(
                command.spiral_endpoint(),
                command.spiral_spfs_endpoint(),
                command.project_id(),
            )
            .with_client_credentials(
                command.client_id(),
                command.client_secret().unwrap_or_default(),
            )
            .with_clear_client_secret(command.client_secret() == Some(""))
            .with_huggingface_token(command.credentials.huggingface_token.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> UpdateDataSettingsCommand {
        UpdateDataSettingsCommand {
            spiral_endpoint: " https://api.spiraldb.dev/ ".to_owned(),
            spiral_spfs_endpoint: " https://spfs.spiraldb.dev/ ".to_owned(),
            project_id: " project ".to_owned(),
            credentials: ConnectionCredentialUpdate::default(),
            training_endpoint: " http://127.0.0.1:50051 ".to_owned(),
            video_enabled: true,
        }
    }

    #[test]
    fn settings_validation_normalizes_all_destinations() {
        let command = settings().validated().unwrap();
        assert_eq!(command.spiral_endpoint(), "https://api.spiraldb.dev");
        assert_eq!(command.spiral_spfs_endpoint(), "https://spfs.spiraldb.dev");
        assert_eq!(command.project_id(), "project");
        assert_eq!(command.training_endpoint(), "http://127.0.0.1:50051");
    }

    #[test]
    fn huggingface_settings_preserve_replace_and_remove_the_private_token() {
        use spiral_robotics_common::settings::SpiralConnectionUpdate;
        let previous = spiral_robotics_storage::SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".into(),
            "https://spfs.spiraldb.dev".into(),
        )
        .unwrap()
        .with_project("project")
        .unwrap()
        .with_huggingface_token(Some("hf_saved".into()));
        for (change, expected) in [
            (None, Some("hf_saved")),
            (Some(" hf_replacement ".to_owned()), Some("hf_replacement")),
            (Some(String::new()), None),
        ] {
            let update = SpiralConnectionUpdate::new(
                previous.endpoint(),
                previous.spfs_endpoint(),
                "project",
            )
            .with_huggingface_token(change.clone());
            let decoded: SpiralConnectionUpdate =
                serde_json::from_str(&serde_json::to_string(&update).unwrap()).unwrap();
            let resolved = ProjectConnectionUpdate::from(decoded)
                .resolve(&previous)
                .unwrap();
            assert_eq!(resolved.huggingface_token(), expected);
            let command = settings()
                .with_huggingface_token(change)
                .validated()
                .unwrap();
            assert!(!format!("{command:?}").contains("hf_replacement"));
            assert_eq!(
                ProjectConnectionUpdate::from(&command)
                    .resolve(&previous)
                    .unwrap()
                    .huggingface_token(),
                expected
            );
        }
    }

    #[test]
    fn connection_repairs_preserve_credentials_unless_the_edit_explicitly_replaces_them() {
        use spiral_robotics_common::settings::SpiralConnectionUpdate;
        use spiral_robotics_storage::{SpiralConnectionSettings, SpiralCredentials};
        let previous = SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".into(),
            "https://spfs.spiraldb.dev".into(),
        )
        .unwrap()
        .with_project("old")
        .unwrap()
        .with_credentials(Some(SpiralCredentials::new(
            "client".into(),
            "saved-secret".into(),
        )));
        let update = SpiralConnectionUpdate::new(
            "https://api.spiraldb.dev",
            "https://spfs.spiraldb.dev",
            "new",
        )
        .with_client_credentials("client", "");
        let repaired = ProjectConnectionUpdate::from(update.clone())
            .resolve(&previous)
            .unwrap();
        assert_eq!(repaired.project_id(), Some("new"));
        assert!(repaired.credentials() == previous.credentials());
        assert!(
            ProjectConnectionUpdate::from(update.clone().with_client_credentials("different", ""))
                .resolve(&previous)
                .is_err()
        );
        assert!(
            ProjectConnectionUpdate::from(update.with_clear_client_secret(true))
                .resolve(&previous)
                .unwrap()
                .credentials()
                .is_none()
        );
    }

    #[test]
    fn invalid_settings_are_rejected_before_project_preparation() {
        let mut invalid = settings();
        invalid.project_id = "not/a/project".to_owned();
        assert!(invalid.validated().is_err());
        let mut invalid = settings();
        invalid.spiral_endpoint = "file:///tmp/spiral".to_owned();
        assert!(invalid.validated().is_err());
        let mut invalid = settings();
        invalid.spiral_spfs_endpoint = "file:///tmp/spfs".to_owned();
        assert!(invalid.validated().is_err());
        let mut invalid = settings();
        invalid.training_endpoint = "ftp://runtime.example".to_owned();
        assert!(invalid.validated().is_err());
    }
}
