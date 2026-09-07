//! Native policy-rollout contracts.

use serde::{Deserialize, Serialize};

/// Operator-approved launch parameters for one backend-owned policy session.
pub const DEFAULT_ROLLOUT_DURATION_SECONDS: u32 = 30;
pub const MAX_ROLLOUT_DURATION_SECONDS: u32 = 600;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RolloutConfig {
    run_id: String,
    checkpoint_step: u64,
    robot: String,
    /// Time spent actively applying policy actions, excluding load and warm-up.
    #[serde(default = "RolloutConfig::default_duration_seconds")]
    duration_seconds: u32,
}

impl RolloutConfig {
    const fn default_duration_seconds() -> u32 {
        DEFAULT_ROLLOUT_DURATION_SECONDS
    }

    pub fn new(run_id: impl Into<String>, checkpoint_step: u64, robot: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            checkpoint_step,
            robot: robot.into(),
            duration_seconds: DEFAULT_ROLLOUT_DURATION_SECONDS,
        }
    }
    pub const fn with_duration(mut self, duration_seconds: u32) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub const fn checkpoint_step(&self) -> u64 {
        self.checkpoint_step
    }
    pub fn robot(&self) -> &str {
        &self.robot
    }
    pub const fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }
}

/// Authoritative lifecycle of the native inference and motor-control task.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
        matches!(
            self,
            Self::Loading | Self::WarmingUp | Self::Running | Self::Stopping
        )
    }
}

/// Measurements accumulated by the active native rollout session.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutMetrics {
    inference_count: u64,
    command_count: u64,
    missed_deadlines: u64,
    last_latency_ms: Option<f64>,
    last_model_ms: Option<f64>,
    mean_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
}

impl RolloutMetrics {
    pub const fn new(inference_count: u64, command_count: u64, missed_deadlines: u64) -> Self {
        Self {
            inference_count,
            command_count,
            missed_deadlines,
            last_latency_ms: None,
            last_model_ms: None,
            mean_latency_ms: None,
            max_latency_ms: None,
        }
    }
    pub const fn with_latest_timings(
        mut self,
        last_latency_ms: Option<f64>,
        last_model_ms: Option<f64>,
    ) -> Self {
        self.last_latency_ms = last_latency_ms;
        self.last_model_ms = last_model_ms;
        self
    }
    pub const fn with_latency_summary(
        mut self,
        mean_latency_ms: Option<f64>,
        max_latency_ms: Option<f64>,
    ) -> Self {
        self.mean_latency_ms = mean_latency_ms;
        self.max_latency_ms = max_latency_ms;
        self
    }
    pub const fn inference_count(&self) -> u64 {
        self.inference_count
    }
    pub const fn command_count(&self) -> u64 {
        self.command_count
    }
    pub const fn missed_deadlines(&self) -> u64 {
        self.missed_deadlines
    }
    pub const fn last_latency_ms(&self) -> Option<f64> {
        self.last_latency_ms
    }
    pub const fn last_model_ms(&self) -> Option<f64> {
        self.last_model_ms
    }
    pub const fn mean_latency_ms(&self) -> Option<f64> {
        self.mean_latency_ms
    }
    pub const fn max_latency_ms(&self) -> Option<f64> {
        self.max_latency_ms
    }
}

/// Browser-safe state for rollout. Model bytes and device handles stay server-side.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutSnapshot {
    #[serde(flatten)]
    identity: RolloutIdentity,
    #[serde(flatten)]
    lifecycle: RolloutLifecycle,
    metrics: RolloutMetrics,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RolloutIdentity {
    session_id: Option<String>,
    run_id: Option<String>,
    checkpoint_step: Option<u64>,
    robot: Option<String>,
    #[serde(default)]
    duration_seconds: Option<u32>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RolloutLifecycle {
    phase: RolloutPhase,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    message: Option<String>,
    #[serde(default)]
    warning: Option<String>,
}

impl RolloutSnapshot {
    pub fn new(session_id: Option<String>, run_id: Option<String>) -> Self {
        Self {
            identity: RolloutIdentity {
                session_id,
                run_id,
                checkpoint_step: None,
                robot: None,
                duration_seconds: None,
            },
            lifecycle: RolloutLifecycle::default(),
            metrics: RolloutMetrics::default(),
        }
    }

    pub fn with_robot(mut self, robot: Option<String>) -> Self {
        self.identity.robot = robot;
        self
    }

    pub const fn with_checkpoint(mut self, checkpoint_step: Option<u64>) -> Self {
        self.identity.checkpoint_step = checkpoint_step;
        self
    }

    pub const fn with_duration(mut self, duration_seconds: Option<u32>) -> Self {
        self.identity.duration_seconds = duration_seconds;
        self
    }

    pub const fn with_lifecycle(mut self, phase: RolloutPhase, started_ms: Option<i64>) -> Self {
        self.lifecycle.phase = phase;
        self.lifecycle.started_ms = started_ms;
        self
    }

    pub fn with_completion(mut self, finished_ms: Option<i64>, message: Option<String>) -> Self {
        self.lifecycle.finished_ms = finished_ms;
        self.lifecycle.message = message;
        self
    }

    pub fn with_warning(mut self, warning: Option<String>) -> Self {
        self.lifecycle.warning = warning;
        self
    }

    pub const fn with_metrics(mut self, metrics: RolloutMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    pub const fn phase(&self) -> RolloutPhase {
        self.lifecycle.phase
    }
    pub fn session_id(&self) -> Option<&str> {
        self.identity.session_id.as_deref()
    }
    pub fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    pub const fn checkpoint_step(&self) -> Option<u64> {
        self.identity.checkpoint_step
    }
    pub fn robot(&self) -> Option<&str> {
        self.identity.robot.as_deref()
    }
    pub const fn duration_seconds(&self) -> Option<u32> {
        self.identity.duration_seconds
    }
    pub const fn started_ms(&self) -> Option<i64> {
        self.lifecycle.started_ms
    }
    pub const fn finished_ms(&self) -> Option<i64> {
        self.lifecycle.finished_ms
    }
    pub fn message(&self) -> Option<&str> {
        self.lifecycle.message.as_deref()
    }
    pub fn warning(&self) -> Option<&str> {
        self.lifecycle.warning.as_deref()
    }
    pub const fn metrics(&self) -> &RolloutMetrics {
        &self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_rollout_configs_receive_the_safe_default_duration() {
        let config: RolloutConfig = serde_json::from_value(serde_json::json!({
            "run_id": "run-1",
            "checkpoint_step": 20_000,
            "robot": "follower"
        }))
        .expect("duration defaults for an older client");

        assert_eq!(config.duration_seconds(), DEFAULT_ROLLOUT_DURATION_SECONDS);
    }
}
