use serde::{Deserialize, Serialize};
use spiral_robotics_common::training::{TrainingConfig, TrainingRunHistory, TrainingRunStatus};

use crate::CURRENT_SPIRAL_SCHEMA_VERSION;

/// The two services required to read one SpiralDB project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpiralServiceEndpoints {
    api_url: String,
    spfs_url: String,
}

impl SpiralServiceEndpoints {
    pub fn new(api_url: impl Into<String>, spfs_url: impl Into<String>) -> Self {
        let endpoints = Self {
            api_url: api_url.into(),
            spfs_url: spfs_url.into(),
        };
        assert!(
            !endpoints.api_url.trim().is_empty(),
            "Spiral API URL must not be empty"
        );
        assert!(
            !endpoints.spfs_url.trim().is_empty(),
            "SPFS URL must not be empty"
        );
        endpoints
    }
}

/// One immutable connection selection sent only to the training worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpiralJobConnection {
    api_url: String,
    spfs_url: String,
    project_id: String,
    table_prefix: String,
}

impl SpiralJobConnection {
    pub fn new(
        endpoints: SpiralServiceEndpoints,
        project_id: impl Into<String>,
        table_prefix: impl Into<String>,
    ) -> Self {
        let connection = Self {
            api_url: endpoints.api_url,
            spfs_url: endpoints.spfs_url,
            project_id: project_id.into(),
            table_prefix: table_prefix.into(),
        };
        assert!(
            !connection.project_id.trim().is_empty(),
            "Spiral project ID must not be empty"
        );
        assert!(
            !connection.table_prefix.trim().is_empty(),
            "Studio table prefix must not be empty"
        );
        connection
    }
}

/// One run's job as Studio asks a worker to start it: the run configuration
/// exactly as the operator admitted it, and where its data lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobRequest {
    job_id: String,
    spiral: SpiralJobConnection,
    spiral_schema_version: u32,
    config: TrainingConfig,
}

impl TrainingJobRequest {
    pub fn new(
        job_id: impl Into<String>,
        spiral: SpiralJobConnection,
        config: TrainingConfig,
    ) -> Self {
        let request = Self {
            job_id: job_id.into(),
            spiral,
            spiral_schema_version: CURRENT_SPIRAL_SCHEMA_VERSION,
            config,
        };
        assert!(
            !request.job_id.trim().is_empty(),
            "job ID must not be empty"
        );
        request
    }
}

/// Where one of a worker's jobs is now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobState {
    job_id: String,
    #[serde(flatten)]
    status: TrainingRunStatus,
}

impl TrainingJobState {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn into_status(self) -> TrainingRunStatus {
        self.status
    }
}

/// A job's state together with the metrics and log tail its worker retains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobHistory {
    #[serde(flatten)]
    state: TrainingJobState,
    #[serde(flatten)]
    history: TrainingRunHistory,
}

impl TrainingJobHistory {
    pub fn into_parts(self) -> (TrainingJobState, TrainingRunHistory) {
        (self.state, self.history)
    }
}
