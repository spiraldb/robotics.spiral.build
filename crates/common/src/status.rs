//! Project availability independent of feature-owned state.

use crate::command_failure::CommandFailureCode;
use serde::{Deserialize, Serialize};

/// Lifecycle of the backend services needed by the browser application.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendReadinessState {
    #[default]
    Bootstrapping,
    Ready,
    Degraded,
}

/// Browser-visible readiness of the backend and its Spiral catalog boundary.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendReadiness {
    state: BackendReadinessState,
    /// A Spiral destination is selected and data-plane requests can be attempted.
    /// Connectivity failures are reported by the requesting data feature.
    catalog_available: bool,
    /// Short operator-facing description of the current phase or problem.
    detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_code: Option<CommandFailureCode>,
}

impl Default for BackendReadiness {
    fn default() -> Self {
        Self {
            state: BackendReadinessState::Bootstrapping,
            catalog_available: false,
            detail: "Starting Studio services".to_owned(),
            failure_code: None,
        }
    }
}

impl BackendReadiness {
    pub fn new(
        state: BackendReadinessState,
        catalog_available: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            state,
            catalog_available,
            detail: detail.into(),
            failure_code: None,
        }
    }
    pub const fn state(&self) -> BackendReadinessState {
        self.state
    }
    pub const fn studio_available(&self) -> bool {
        matches!(self.state, BackendReadinessState::Ready) || self.catalog_available
    }

    pub const fn catalog_available(&self) -> bool {
        self.catalog_available
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
    pub fn with_failure_code(mut self, code: CommandFailureCode) -> Self {
        self.failure_code = Some(code);
        self
    }
    pub const fn failure_code(&self) -> Option<CommandFailureCode> {
        self.failure_code
    }
}
