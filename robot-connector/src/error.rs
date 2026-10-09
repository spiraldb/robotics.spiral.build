//! Errors owned by robot connector policy, independent of any delivery protocol.

use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};

/// Stable failure categories the HTTP adapter presents as responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
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
            | Self::External(message)
            | Self::Internal(message) => message,
        }
    }

    pub(crate) const fn code(&self) -> CommandFailureCode {
        match self {
            Self::InvalidInput(_) => CommandFailureCode::InvalidInput,
            Self::NotFound(_) => CommandFailureCode::NotFound,
            Self::Conflict(_) => CommandFailureCode::Conflict,
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

impl From<spiral_robotics_recording::EpisodeError> for OperationError {
    fn from(error: spiral_robotics_recording::EpisodeError) -> Self {
        let message = error.to_string();
        match error {
            spiral_robotics_recording::EpisodeError::InvalidId(_)
            | spiral_robotics_recording::EpisodeError::Invalid(_) => Self::InvalidInput(message),
            spiral_robotics_recording::EpisodeError::NotFound(_) => Self::NotFound(message),
        }
    }
}
