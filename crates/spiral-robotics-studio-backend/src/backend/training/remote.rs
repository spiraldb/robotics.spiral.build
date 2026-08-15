//! Settings-backed HTTP and Arrow IPC training adapter.

use std::sync::Arc;

use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingSession,
    TrainingSessionRequest,
};
use spiral_robotics_training_http::HttpTrainingBackend;

use crate::backend::storage::settings::StudioPreferencesStore;

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

    fn create_session(
        &self,
        request: TrainingSessionRequest,
    ) -> Result<Box<dyn TrainingSession>, TrainingBackendError> {
        self.selected().create_session(request)
    }
}
