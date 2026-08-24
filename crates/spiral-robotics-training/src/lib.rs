//! Backend-neutral model-training boundary.
//!
//! Studio owns orchestration and the visible run history. A job backend owns
//! SpiralDB reads, model execution, telemetry, and artifact publication.
//! Rollout inference has a separate HTTP + Arrow boundary; no framework tensor
//! type crosses this training seam.

use std::fmt;

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/training_boundary.rs"));

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingBackendInfo {
    #[serde(default)]
    pub api_id: String,
    pub api_version: u32,
    #[serde(default)]
    pub api_revision: u32,
    #[serde(default)]
    pub spiral_schema_minimum: u32,
    #[serde(default)]
    pub spiral_schema_maximum: u32,
    pub id: String,
    pub display_name: String,
    pub runtime: String,
    pub devices: Vec<String>,
}

impl TrainingBackendInfo {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {}",
            self.display_name,
            self.runtime,
            self.devices.join(" + ")
        )
    }

    /// Return a precise incompatibility instead of relying on scattered exact
    /// integer comparisons. Minor revisions are additive: a newer runtime of
    /// the same major may serve an older Studio client.
    pub fn compatibility_error(&self, spiral_schema_version: u32) -> Option<String> {
        if self.api_version != TRAINING_BACKEND_API_VERSION {
            return Some(format!(
                "training backend API major {} is incompatible with Studio major {}",
                self.api_version, TRAINING_BACKEND_API_VERSION
            ));
        }
        if self.api_id != TRAINING_BACKEND_API_ID {
            return Some(format!(
                "training backend API ID {:?} is incompatible with {:?}",
                self.api_id, TRAINING_BACKEND_API_ID
            ));
        }
        if !revision_is_compatible(self.api_revision, TRAINING_BACKEND_API_REVISION) {
            return Some(format!(
                "training backend API {}.{} is older than Studio's required {}.{}",
                self.api_version,
                self.api_revision,
                TRAINING_BACKEND_API_VERSION,
                TRAINING_BACKEND_API_REVISION
            ));
        }
        if self.spiral_schema_minimum > self.spiral_schema_maximum {
            return Some(format!(
                "training backend advertised an invalid Spiral schema range {}..={}",
                self.spiral_schema_minimum, self.spiral_schema_maximum
            ));
        }
        if !(self.spiral_schema_minimum..=self.spiral_schema_maximum)
            .contains(&spiral_schema_version)
        {
            return Some(format!(
                "training backend supports Spiral schema {}..={}, but Studio uses {}",
                self.spiral_schema_minimum, self.spiral_schema_maximum, spiral_schema_version
            ));
        }
        None
    }
}

const fn revision_is_compatible(runtime: u32, required: u32) -> bool {
    runtime >= required
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingInitialization {
    Random,
    PretrainedBackbone,
}

/// One immutable connection selection sent only to the training worker. The
/// client secret is write-only control-plane material and must never appear in
/// job snapshots, logs, or telemetry.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SpiralJobConnection {
    pub api_url: String,
    pub spfs_url: String,
    pub project_id: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl fmt::Debug for SpiralJobConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpiralJobConnection")
            .field("api_url", &self.api_url)
            .field("spfs_url", &self.spfs_url)
            .field("project_id", &self.project_id)
            .field("namespace", &self.namespace)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingJobConfig {
    pub camera_streams: Vec<String>,
    pub image_width: usize,
    pub image_height: usize,
    pub video_decoder: String,
    pub hidden_dimensions: usize,
    pub latent_dimensions: usize,
    pub attention_heads: usize,
    pub feedforward_dimensions: usize,
    pub encoder_layers: usize,
    pub vae_encoder_layers: usize,
    pub decoder_layers: usize,
    pub kl_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobRequest {
    pub job_id: String,
    pub run_name: String,
    pub spiral: SpiralJobConnection,
    pub sql: String,
    pub state_column: String,
    pub action_column: String,
    /// The relational contract used to select this job's data. The worker
    /// records the received value in every checkpoint rather than maintaining
    /// a second schema-version constant.
    pub spiral_schema_version: u32,
    pub device: String,
    pub initialization: TrainingInitialization,
    pub steps: u64,
    pub batch_size: usize,
    pub validation_split: f64,
    pub learning_rate: f64,
    pub checkpoint_every: u64,
    pub chunk_size: usize,
    pub seed: u64,
    pub act: ActTrainingJobConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingJobPhase {
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl TrainingJobPhase {
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobResolved {
    pub camera_streams: Vec<String>,
    pub state_dimensions: usize,
    pub action_dimensions: usize,
    pub image_width: usize,
    pub image_height: usize,
    pub sample_hz: f64,
    pub robot_type: String,
    pub joint_names: Vec<String>,
    pub parameter_count: usize,
    pub device: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobMetric {
    pub step: u64,
    pub train_loss: f64,
    pub validation_loss: Option<f64>,
    pub learning_rate: f64,
    pub examples_per_second: f64,
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub loader_ms: Option<f64>,
    pub device_copy_ms: Option<f64>,
    pub model_ms: Option<f64>,
    pub elapsed_seconds: f64,
    pub event: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobCheckpoint {
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub byte_size: u64,
    pub recommended_action_steps: usize,
    pub sample_hz: f64,
}

/// Append-only worker history position. Callers send the last observed cursor
/// so polling transfers only new state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobCursor {
    pub metric_step: u64,
    pub checkpoint_step: u64,
    pub log_offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobLog {
    pub offset: u64,
    pub line: String,
}

/// Current worker state plus replay deltas after the requested cursor.
///
/// Collections are append-only but retained within worker-side bounds. A
/// caller that falls behind those bounds receives `history_gap` alongside the
/// oldest still-available deltas and can continue from `cursor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobSnapshot {
    pub job_id: String,
    pub phase: TrainingJobPhase,
    pub current_step: u64,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    pub resolved: Option<TrainingJobResolved>,
    pub metrics: Vec<TrainingJobMetric>,
    pub checkpoints: Vec<TrainingJobCheckpoint>,
    pub logs: Vec<TrainingJobLog>,
    /// Cursor after all deltas in this response.
    #[serde(default)]
    pub cursor: TrainingJobCursor,
    /// Present when the requested cursor predates bounded worker retention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_gap: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TrainingBackendError {
    message: String,
}

impl TrainingBackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Factory and health boundary owned by Studio as `Arc<dyn TrainingBackend>`.
///
/// Calls are made from worker threads, so a remote implementation may perform
/// blocking transport I/O without blocking Studio's async control loop.
pub trait TrainingBackend: Send + Sync {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError>;

    fn create_job(
        &self,
        _request: TrainingJobRequest,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }

    fn job(
        &self,
        _job_id: &str,
        _cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }

    fn stop_job(
        &self,
        _job_id: &str,
        _cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    struct RemoteBackend;

    impl TrainingBackend for RemoteBackend {
        fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
            Ok(TrainingBackendInfo {
                api_id: TRAINING_BACKEND_API_ID.to_owned(),
                api_version: TRAINING_BACKEND_API_VERSION,
                api_revision: TRAINING_BACKEND_API_REVISION,
                spiral_schema_minimum: TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION,
                spiral_schema_maximum: TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION,
                id: "remote.test".to_owned(),
                display_name: "Remote test backend".to_owned(),
                runtime: "HTTP".to_owned(),
                devices: vec!["remote".to_owned()],
            })
        }
    }

    #[test]
    fn boundary_is_object_safe_and_remote_implementable() {
        let backend: std::sync::Arc<dyn TrainingBackend> = std::sync::Arc::new(RemoteBackend);
        assert_eq!(backend.health().unwrap().id, "remote.test");
    }

    #[test]
    fn job_connection_debug_output_redacts_the_client_secret() {
        let connection = SpiralJobConnection {
            api_url: "https://api.spiraldb.dev".to_owned(),
            spfs_url: "https://spfs.spiraldb.dev".to_owned(),
            project_id: "project".to_owned(),
            namespace: "spiral_robotics".to_owned(),
            client_id: Some("client-id".to_owned()),
            client_secret: Some("do-not-log-this".to_owned()),
        };
        let debug = format!("{connection:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("do-not-log-this"));
    }

    fn backend_info(
        revision: u32,
        schema_minimum: u32,
        schema_maximum: u32,
    ) -> TrainingBackendInfo {
        TrainingBackendInfo {
            api_id: TRAINING_BACKEND_API_ID.to_owned(),
            api_version: TRAINING_BACKEND_API_VERSION,
            api_revision: revision,
            spiral_schema_minimum: schema_minimum,
            spiral_schema_maximum: schema_maximum,
            id: "remote.test".to_owned(),
            display_name: "Remote test backend".to_owned(),
            runtime: "HTTP".to_owned(),
            devices: vec!["remote".to_owned()],
        }
    }

    #[test]
    fn compatibility_uses_major_minor_and_schema_range_rules() {
        assert!(revision_is_compatible(2, 1));
        assert!(!revision_is_compatible(1, 2));
        let current_schema = TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION;
        assert_eq!(
            backend_info(
                TRAINING_BACKEND_API_REVISION,
                current_schema,
                current_schema
            )
            .compatibility_error(current_schema),
            None
        );
        assert_eq!(
            backend_info(
                TRAINING_BACKEND_API_REVISION + 1,
                current_schema,
                current_schema
            )
            .compatibility_error(current_schema),
            None,
            "a newer additive runtime revision remains compatible"
        );

        let mut wrong_major = backend_info(
            TRAINING_BACKEND_API_REVISION,
            current_schema,
            current_schema,
        );
        wrong_major.api_version += 1;
        assert!(
            wrong_major
                .compatibility_error(current_schema)
                .is_some_and(|error| error.contains("major"))
        );

        assert!(
            backend_info(
                TRAINING_BACKEND_API_REVISION,
                current_schema + 1,
                current_schema + 1
            )
            .compatibility_error(current_schema)
            .is_some_and(|error| error.contains("Spiral schema"))
        );
    }

    #[test]
    fn canonical_wire_fixtures_round_trip_through_rust_contracts() {
        let health: TrainingBackendInfo =
            serde_json::from_str(include_str!("../fixtures/health.json")).unwrap();
        assert_eq!(health.api_id, TRAINING_BACKEND_API_ID);
        assert_eq!(health.api_version, TRAINING_BACKEND_API_VERSION);
        assert_eq!(health.api_revision, TRAINING_BACKEND_API_REVISION);
        assert_eq!(
            health.spiral_schema_minimum,
            TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION
        );
        assert_eq!(
            health.spiral_schema_maximum,
            TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION
        );
        let request: TrainingJobRequest =
            serde_json::from_str(include_str!("../fixtures/job-request.json")).unwrap();
        assert_eq!(
            request.spiral_schema_version,
            TRAINING_CURRENT_SPIRAL_SCHEMA_VERSION
        );

        assert_fixture::<TrainingBackendInfo>(include_str!("../fixtures/health.json"));
        assert_fixture::<TrainingJobRequest>(include_str!("../fixtures/job-request.json"));
        assert_fixture::<TrainingJobSnapshot>(include_str!("../fixtures/job-snapshot.json"));
    }

    fn assert_fixture<T>(source: &str)
    where
        T: DeserializeOwned + Serialize,
    {
        let expected: serde_json::Value = serde_json::from_str(source).unwrap();
        let contract: T = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(contract).unwrap(), expected);
    }
}
