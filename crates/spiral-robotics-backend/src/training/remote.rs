//! Settings-backed HTTP and Arrow IPC training adapter.

use std::sync::Arc;

use spiral_robotics_storage::settings::StudioPreferencesStore;
use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingJobCursor,
    TrainingJobRequest, TrainingJobSnapshot,
};

use crate::adapters::compute_runtime::HttpTrainingBackend;

/// Resolve the selected endpoint for every probe and new session. Existing
/// sessions retain their selected HTTP endpoint when Settings changes.
pub(crate) struct ConfiguredHttpTrainingBackend {
    preferences: Arc<StudioPreferencesStore>,
}

impl ConfiguredHttpTrainingBackend {
    pub(crate) fn new(preferences: Arc<StudioPreferencesStore>) -> Self {
        Self { preferences }
    }

    fn selected(&self) -> HttpTrainingBackend {
        HttpTrainingBackend::new(self.preferences.get().training.endpoint)
    }
}

impl TrainingBackend for ConfiguredHttpTrainingBackend {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
        self.selected().health()
    }

    fn create_job(
        &self,
        request: TrainingJobRequest,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected().create_job(request)
    }

    fn job(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected().job(job_id, cursor)
    }

    fn stop_job(
        &self,
        job_id: &str,
        cursor: TrainingJobCursor,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        self.selected().stop_job(job_id, cursor)
    }
}
