//! Settings-backed HTTP and Arrow IPC training adapter.

use std::sync::Arc;

use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingJobCursor,
    TrainingJobRequest, TrainingJobSnapshot,
};

use crate::adapters::compute_runtime::HttpTrainingBackend;

/// Resolve the selected endpoint on every call. Running jobs are followed
/// through it too, so the endpoint changes only between runs.
pub(crate) struct ConfiguredHttpTrainingBackend {
    preferences: Arc<crate::data::preferences::RuntimePreferences>,
}

impl ConfiguredHttpTrainingBackend {
    pub(crate) fn new(preferences: Arc<crate::data::preferences::RuntimePreferences>) -> Self {
        Self { preferences }
    }

    fn selected(&self) -> Result<HttpTrainingBackend, TrainingBackendError> {
        Ok(HttpTrainingBackend::new(
            self.preferences
                .loaded()
                .map_err(TrainingBackendError::new)?
                .training()
                .endpoint(),
        ))
    }
}

impl TrainingBackend for ConfiguredHttpTrainingBackend {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
        self.selected()?.health()
    }

    fn create_job(
        &self,
        request: TrainingJobRequest,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected()?.create_job(request)
    }

    fn job_snapshot(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected()?.job_snapshot(job_id, cursor)
    }

    fn stop_job(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected()?.stop_job(job_id, cursor)
    }
}
