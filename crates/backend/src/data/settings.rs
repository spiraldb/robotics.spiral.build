//! Validation for project connection and runtime preferences.

use spiral_robotics_common::settings::{DataSettings, SettingsUpdate};
use url::Url;

use crate::OperationError;
use crate::projects::OpenProject;
use std::sync::Arc;

pub(crate) struct HttpEndpoint(String);

impl HttpEndpoint {
    pub(crate) fn parse(endpoint: &str, label: &str) -> Result<Self, OperationError> {
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

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpdateDataSettingsCommand {
    spiral_endpoint: String,
    spiral_spfs_endpoint: String,
    project_id: String,
    huggingface_token: HuggingFaceTokenUpdate,
    training_endpoint: String,
    video_enabled: bool,
}

/// Omitted keeps the saved token; empty removes it.
#[derive(Clone, Default, Eq, PartialEq)]
struct HuggingFaceTokenUpdate(Option<String>);

impl std::fmt::Debug for HuggingFaceTokenUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HuggingFaceTokenUpdate")
            .field(&self.0.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Saved settings, and the project the save switched to when it changed one.
pub(crate) struct DataSettingsUpdate {
    settings: DataSettings,
    switched_to: Option<Arc<OpenProject>>,
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
            huggingface_token: HuggingFaceTokenUpdate::default(),
            training_endpoint: training_endpoint.into(),
            video_enabled: false,
        }
    }

    pub(super) fn with_spfs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.spiral_spfs_endpoint = endpoint.into();
        self
    }

    pub(super) fn with_huggingface_token(mut self, token: Option<String>) -> Self {
        self.huggingface_token = HuggingFaceTokenUpdate(token);
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
    pub(crate) fn training_endpoint(&self) -> &str {
        &self.training_endpoint
    }
    pub(crate) fn video_enabled(&self) -> bool {
        self.video_enabled
    }
}

impl From<SettingsUpdate> for UpdateDataSettingsCommand {
    fn from(update: SettingsUpdate) -> Self {
        let (connection, training, video) = update.into_parts();
        let (endpoint, spfs, project, huggingface_token) = connection.into_parts();
        Self::new(endpoint, project, training.endpoint())
            .with_spfs_endpoint(spfs)
            .with_huggingface_token(huggingface_token)
            .with_video_enabled(video.enabled())
    }
}

impl DataSettingsUpdate {
    pub(crate) fn new(settings: DataSettings, switched_to: Option<Arc<OpenProject>>) -> Self {
        Self {
            settings,
            switched_to,
        }
    }

    pub(super) fn into_parts(self) -> (DataSettings, Option<Arc<OpenProject>>) {
        (self.settings, self.switched_to)
    }
}

impl UpdateDataSettingsCommand {
    pub(crate) fn validated(mut self) -> Result<Self, OperationError> {
        self.spiral_endpoint =
            HttpEndpoint::parse(&self.spiral_endpoint, "Spiral API endpoint")?.into_string();
        self.spiral_spfs_endpoint =
            HttpEndpoint::parse(&self.spiral_spfs_endpoint, "Spiral SpFS endpoint")?.into_string();
        self.project_id = self.project_id.trim().to_owned();
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

        self.training_endpoint =
            HttpEndpoint::parse(&self.training_endpoint, "Training runtime endpoint")?
                .into_string();

        Ok(self)
    }
}

impl From<&UpdateDataSettingsCommand> for spiral_robotics_common::settings::SpiralConnectionUpdate {
    fn from(command: &UpdateDataSettingsCommand) -> Self {
        spiral_robotics_common::settings::SpiralConnectionUpdate::new(
            command.spiral_endpoint(),
            command.spiral_spfs_endpoint(),
            command.project_id(),
        )
        .with_huggingface_token(command.huggingface_token.0.clone())
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
            huggingface_token: HuggingFaceTokenUpdate::default(),
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
    fn settings_convert_to_connection_updates_without_exposing_secrets() {
        use spiral_robotics_common::settings::SpiralConnectionUpdate;

        for token in [None, Some("hf_replacement".to_owned()), Some(String::new())] {
            let command = settings()
                .with_huggingface_token(token.clone())
                .validated()
                .unwrap();
            assert!(!format!("{command:?}").contains("hf_replacement"));
            let (_, _, project, converted_token) =
                SpiralConnectionUpdate::from(&command).into_parts();
            assert_eq!(project, "project");
            assert_eq!(converted_token, token);
        }
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
