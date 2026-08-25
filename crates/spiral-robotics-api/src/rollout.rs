//! Native policy-rollout contracts.

use serde::{Deserialize, Serialize};
use spiral_robotics_core::operation::OperationLifecycle;

/// Operator-approved launch parameters for one backend-owned policy session.
pub use spiral_robotics_core::operation::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_ACTION_STEPS, MAX_ROLLOUT_DURATION_SECONDS,
    MIN_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRolloutRequest {
    pub run_id: String,
    pub checkpoint_step: u64,
    pub robot: String,
    /// Time spent actively applying policy actions, excluding load and warm-up.
    #[serde(default = "default_rollout_duration_seconds")]
    pub duration_seconds: u32,
}

const fn default_rollout_duration_seconds() -> u32 {
    DEFAULT_ROLLOUT_DURATION_SECONDS
}

/// Authoritative lifecycle of the native inference and motor-control task.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutPhase {
    #[default]
    Idle,
    Loading,
    WarmingUp,
    Running,
    Stopping,
    Failed,
}

impl RolloutPhase {
    pub const fn active(self) -> bool {
        match self {
            Self::Loading => OperationLifecycle::Pending,
            Self::WarmingUp | Self::Running => OperationLifecycle::Running,
            Self::Stopping => OperationLifecycle::Stopping,
            Self::Idle | Self::Failed => OperationLifecycle::Finished,
        }
        .active()
    }
}

/// Measurements accumulated by the active native rollout session.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutMetrics {
    pub inference_count: u64,
    pub command_count: u64,
    pub missed_deadlines: u64,
    pub last_latency_ms: Option<f64>,
    pub last_model_ms: Option<f64>,
    pub mean_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
}

/// Browser-safe state for rollout. Model bytes and device handles stay server-side.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutSnapshot {
    pub phase: RolloutPhase,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub checkpoint_step: Option<u64>,
    pub robot: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
    pub metrics: RolloutMetrics,
}
