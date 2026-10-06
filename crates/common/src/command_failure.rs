//! Stable command failures shared by the backend and its delivery adapters.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandFailureCode {
    InvalidInput,
    /// The request carries no live Spiral sign-in.
    Unauthenticated,
    NotFound,
    Conflict,
    StaleProject,
    External,
    Transport,
    Internal,
}

impl CommandFailureCode {
    /// The body's code is what tells failures apart. A failure of something
    /// Studio depends on would be a 502, but Cloudflare, in front of hosted
    /// Studio, replaces the body of an origin 502 or 504 with its own error
    /// page, so it travels as a 500 and keeps its message.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidInput => 400,
            Self::Unauthenticated => 401,
            Self::NotFound => 404,
            Self::Conflict | Self::StaleProject => 409,
            Self::Transport => 503,
            Self::External | Self::Internal => 500,
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
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandFailure {}
