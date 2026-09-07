//! Errors owned by application policy, independent of any delivery protocol.

use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};

/// Stable failure categories that delivery adapters may present as HTTP,
/// desktop, CLI, or other transport-specific responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    ProjectTransition(String),
    ProjectUnavailable(String),
    StaleProject(String),
    IncompatibleWorkflow(String),
    External(String),
    Internal(String),
}

impl OperationError {
    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::InvalidInput(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::ProjectTransition(message)
            | Self::ProjectUnavailable(message)
            | Self::StaleProject(message)
            | Self::IncompatibleWorkflow(message)
            | Self::External(message)
            | Self::Internal(message) => message,
        }
    }

    pub(crate) const fn code(&self) -> CommandFailureCode {
        match self {
            Self::InvalidInput(_) => CommandFailureCode::InvalidInput,
            Self::NotFound(_) => CommandFailureCode::NotFound,
            Self::Conflict(_) => CommandFailureCode::Conflict,
            Self::ProjectTransition(_) => CommandFailureCode::ProjectTransition,
            Self::ProjectUnavailable(_) => CommandFailureCode::ProjectUnavailable,
            Self::StaleProject(_) => CommandFailureCode::StaleProject,
            Self::IncompatibleWorkflow(_) => CommandFailureCode::IncompatibleWorkflow,
            Self::External(_) => CommandFailureCode::External,
            Self::Internal(_) => CommandFailureCode::Internal,
        }
    }
}

impl From<OperationError> for CommandFailure {
    fn from(error: OperationError) -> Self {
        Self::new(error.code(), error.message())
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for OperationError {}

impl From<spiral_robotics_storage::episodes::EpisodeError> for OperationError {
    fn from(error: spiral_robotics_storage::episodes::EpisodeError) -> Self {
        let message = error.to_string();
        match error {
            spiral_robotics_storage::episodes::EpisodeError::InvalidId(_)
            | spiral_robotics_storage::episodes::EpisodeError::Invalid(_) => {
                Self::InvalidInput(message)
            }
            spiral_robotics_storage::episodes::EpisodeError::NotFound(_) => Self::NotFound(message),
        }
    }
}

/// Failure returned by every backend command facade.
#[derive(Debug)]
pub(crate) struct BackendCommandError(OperationError);

impl BackendCommandError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self(OperationError::invalid_input(message))
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self(OperationError::conflict(message))
    }
}

impl std::fmt::Display for BackendCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for BackendCommandError {}

impl From<OperationError> for BackendCommandError {
    fn from(error: OperationError) -> Self {
        Self(error)
    }
}

impl From<String> for BackendCommandError {
    fn from(message: String) -> Self {
        Self(OperationError::External(message))
    }
}

impl From<crate::project_session::tasks::BackendTaskFailure> for BackendCommandError {
    fn from(error: crate::project_session::tasks::BackendTaskFailure) -> Self {
        Self(OperationError::Internal(error.to_string()))
    }
}

impl From<BackendCommandError> for CommandFailure {
    fn from(error: BackendCommandError) -> Self {
        error.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_facades_preserve_error_categories() {
        for (error, expected, status) in [
            (
                OperationError::ProjectTransition("arbitrary text".into()),
                CommandFailureCode::ProjectTransition,
                409,
            ),
            (
                OperationError::ProjectUnavailable("arbitrary text".into()),
                CommandFailureCode::ProjectUnavailable,
                503,
            ),
            (
                OperationError::IncompatibleWorkflow("arbitrary text".into()),
                CommandFailureCode::IncompatibleWorkflow,
                422,
            ),
            (
                OperationError::Internal("arbitrary text".into()),
                CommandFailureCode::Internal,
                500,
            ),
            (
                OperationError::StaleProject("arbitrary text".into()),
                CommandFailureCode::StaleProject,
                409,
            ),
        ] {
            let failure = CommandFailure::from(BackendCommandError::from(error));
            assert_eq!(failure.code(), expected);
            assert_eq!(failure.code().http_status(), status);
            assert_eq!(failure.message(), "arbitrary text");
        }
    }
}
