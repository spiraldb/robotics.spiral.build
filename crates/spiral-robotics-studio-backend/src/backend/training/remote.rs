//! Settings-backed HTTP and Arrow IPC training adapter.

use std::sync::Arc;

use spiral_robotics_training::{
    TrainingBackend, TrainingBackendError, TrainingBackendInfo, TrainingSession,
    TrainingSessionRequest,
};
use spiral_robotics_training_http::HttpTrainingBackend;

use crate::backend::storage::settings::DataSettingsStore;

/// Resolve the selected endpoint for every probe and new session. Existing
/// sessions retain their selected HTTP endpoint when Settings changes.
pub(crate) struct ConfiguredHttpTrainingBackend {
    settings: Arc<DataSettingsStore>,
}

impl ConfiguredHttpTrainingBackend {
    pub(crate) fn new(settings: Arc<DataSettingsStore>) -> Self {
        Self { settings }
    }

    fn selected(&self) -> HttpTrainingBackend {
        HttpTrainingBackend::new(self.settings.get().training.endpoint)
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
