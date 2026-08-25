//! Errors owned by application policy, independent of any delivery protocol.

/// Stable failure categories that delivery adapters may present as HTTP,
/// desktop, CLI, or other transport-specific responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    External(String),
    Internal(String),
}

impl OperationError {
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn external(message: impl Into<String>) -> Self {
        Self::External(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    pub fn message(&self) -> &str {
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
