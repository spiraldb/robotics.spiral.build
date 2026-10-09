//! Errors owned by application policy, independent of any delivery protocol.

use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};

/// Stable failure categories the HTTP adapter presents as responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationError {
    InvalidInput(String),
    Unauthenticated(String),
    NotFound(String),
    Conflict(String),
    StaleProject(String),
    External(String),
    /// Something Studio relays to cannot be reached now, such as a robot
    /// connector that is not linked.
    Unavailable(String),
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
            | Self::Unauthenticated(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::StaleProject(message)
            | Self::External(message)
            | Self::Unavailable(message)
            | Self::Internal(message) => message,
        }
    }

    pub(crate) const fn code(&self) -> CommandFailureCode {
        match self {
            Self::InvalidInput(_) => CommandFailureCode::InvalidInput,
            Self::Unauthenticated(_) => CommandFailureCode::Unauthenticated,
            Self::NotFound(_) => CommandFailureCode::NotFound,
            Self::Conflict(_) => CommandFailureCode::Conflict,
            Self::StaleProject(_) => CommandFailureCode::StaleProject,
            Self::External(_) => CommandFailureCode::External,
            Self::Unavailable(_) => CommandFailureCode::Transport,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_preserve_error_categories() {
        for (error, expected, status) in [
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
            // Never 502: Cloudflare would swap the message for its own page.
            (
                OperationError::External("arbitrary text".into()),
                CommandFailureCode::External,
                500,
            ),
            (
                OperationError::Unavailable("arbitrary text".into()),
                CommandFailureCode::Transport,
                503,
            ),
        ] {
            let failure = CommandFailure::from(error);
            assert_eq!(failure.code(), expected);
            assert_eq!(failure.code().http_status(), status);
            assert_eq!(failure.to_string(), "arbitrary text");
        }
    }
}
