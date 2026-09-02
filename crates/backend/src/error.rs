//! Errors owned by application policy, independent of any delivery protocol.

/// Stable failure categories that delivery adapters may present as HTTP,
/// desktop, CLI, or other transport-specific responses.
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
