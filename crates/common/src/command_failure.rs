//! Stable command failures shared by the backend and its delivery adapters.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandFailureCode {
    InvalidInput,
    NotFound,
    Conflict,
    StaleProject,
    IncompatibleWorkflow,
    External,
    Transport,
    #[serde(other)]
    Internal,
}

impl CommandFailureCode {
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidInput => 400,
            Self::NotFound => 404,
            Self::Conflict | Self::StaleProject => 409,
            Self::IncompatibleWorkflow => 422,
            Self::Transport => 503,
            Self::External => 502,
            Self::Internal => 500,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandFailure {
    code: CommandFailureCode,
    message: String,
}

impl CommandFailure {
    pub fn new(code: CommandFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> CommandFailureCode {
        self.code
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandFailure {}
