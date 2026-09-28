use serde::{Deserialize, Serialize};

use crate::{TRAINING_BACKEND_API_ID, TRAINING_BACKEND_API_REVISION, TRAINING_BACKEND_API_VERSION};

#[cfg(test)]
use crate::{
    TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION, TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct TrainingBackendProtocol {
    #[serde(default)]
    api_id: String,
    api_version: u32,
    #[serde(default)]
    api_revision: u32,
    #[serde(default)]
    spiral_schema_minimum: u32,
    #[serde(default)]
    spiral_schema_maximum: u32,
}

impl TrainingBackendProtocol {
    fn is_revision_compatible(&self) -> bool {
        self.api_revision
            .cmp(&TRAINING_BACKEND_API_REVISION)
            .is_ge()
    }

    #[cfg(test)]
    fn fixture(api_revision: u32, schema_minimum: u32, schema_maximum: u32) -> Self {
        Self {
            api_revision,
            spiral_schema_minimum: schema_minimum,
            spiral_schema_maximum: schema_maximum,
            ..Self::current()
        }
    }
    #[cfg(test)]
    fn current() -> Self {
        Self {
            api_id: TRAINING_BACKEND_API_ID.to_owned(),
            api_version: TRAINING_BACKEND_API_VERSION,
            api_revision: TRAINING_BACKEND_API_REVISION,
            spiral_schema_minimum: TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION,
            spiral_schema_maximum: TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION,
        }
    }
}

/// Stable identity reported by a training backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct TrainingBackendIdentity {
    id: String,
    display_name: String,
    runtime: String,
}

/// Health and compatibility information for one training backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingBackendInfo {
    #[serde(flatten)]
    protocol: TrainingBackendProtocol,
    #[serde(flatten)]
    identity: TrainingBackendIdentity,
    devices: Vec<String>,
}

impl TrainingBackendInfo {
    pub(crate) fn api_id(&self) -> &str {
        &self.protocol.api_id
    }

    pub(crate) const fn api_version(&self) -> u32 {
        self.protocol.api_version
    }

    pub(crate) const fn api_revision(&self) -> u32 {
        self.protocol.api_revision
    }

    pub(crate) const fn spiral_schema_minimum(&self) -> u32 {
        self.protocol.spiral_schema_minimum
    }

    pub(crate) const fn spiral_schema_maximum(&self) -> u32 {
        self.protocol.spiral_schema_maximum
    }

    pub fn display_name(&self) -> &str {
        &self.identity.display_name
    }

    pub fn runtime(&self) -> &str {
        &self.identity.runtime
    }

    pub fn devices(&self) -> &[String] {
        &self.devices
    }

    pub fn display_summary(&self) -> String {
        format!(
            "{} · {} · {}",
            self.display_name(),
            self.runtime(),
            self.devices.join(" + ")
        )
    }

    /// Return a precise incompatibility instead of relying on scattered exact
    /// integer comparisons. Minor revisions are additive: a newer runtime of
    /// the same major may serve an older Studio client.
    pub fn compatibility_error(&self, spiral_schema_version: u32) -> Option<String> {
        if self.api_version() != TRAINING_BACKEND_API_VERSION {
            return Some(format!(
                "training backend API major {} is incompatible with Studio major {}",
                self.api_version(),
                TRAINING_BACKEND_API_VERSION
            ));
        }
        if self.api_id() != TRAINING_BACKEND_API_ID {
            return Some(format!(
                "training backend API ID {:?} is incompatible with {:?}",
                self.api_id(),
                TRAINING_BACKEND_API_ID
            ));
        }
        if !self.protocol.is_revision_compatible() {
            return Some(format!(
                "training backend API {}.{} is older than Studio's required {}.{}",
                self.api_version(),
                self.api_revision(),
                TRAINING_BACKEND_API_VERSION,
                TRAINING_BACKEND_API_REVISION
            ));
        }
        if self.spiral_schema_minimum() > self.spiral_schema_maximum() {
            return Some(format!(
                "training backend advertised an invalid Spiral schema range {}..={}",
                self.spiral_schema_minimum(),
                self.spiral_schema_maximum()
            ));
        }
        if !(self.spiral_schema_minimum()..=self.spiral_schema_maximum())
            .contains(&spiral_schema_version)
        {
            return Some(format!(
                "training backend supports Spiral schema {}..={}, but Studio uses {}",
                self.spiral_schema_minimum(),
                self.spiral_schema_maximum(),
                spiral_schema_version
            ));
        }
        None
    }

    #[cfg(test)]
    pub(super) fn test_fixture(
        api_revision: u32,
        schema_minimum: u32,
        schema_maximum: u32,
    ) -> Self {
        Self {
            protocol: TrainingBackendProtocol::fixture(
                api_revision,
                schema_minimum,
                schema_maximum,
            ),
            identity: TrainingBackendIdentity::new("remote.test", "Remote test backend", "HTTP"),
            devices: vec!["remote".to_owned()],
        }
    }

    #[cfg(test)]
    pub(super) fn test_fixture_with_api_version(api_version: u32, schema_version: u32) -> Self {
        let mut fixture = Self::test_fixture(
            TRAINING_BACKEND_API_REVISION,
            schema_version,
            schema_version,
        );
        fixture.protocol.api_version = api_version;
        fixture
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TrainingBackendError {
    message: String,
}

impl TrainingBackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl TrainingBackendIdentity {
    #[cfg(test)]
    pub(super) fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        runtime: impl Into<String>,
    ) -> Self {
        let identity = Self {
            id: id.into(),
            display_name: display_name.into(),
            runtime: runtime.into(),
        };
        assert!(
            !identity.id.trim().is_empty(),
            "backend ID must not be empty"
        );
        assert!(
            !identity.display_name.trim().is_empty(),
            "backend display name must not be empty"
        );
        assert!(
            !identity.runtime.trim().is_empty(),
            "backend runtime description must not be empty"
        );
        identity
    }
}
