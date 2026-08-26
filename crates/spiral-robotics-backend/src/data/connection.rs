//! Authoritative Spiral project-session configuration and transitions.

use std::sync::{Arc, Mutex};

use spiral_robotics_api::settings::{TrainingRuntimeSettings, VideoSettings};
use spiral_robotics_storage::{
    BackendStateStore, SpiralConnectionSettings, SpiralCredentials, StorageManager,
    StudioPreferencesStore,
};

use super::{DataSettingsSnapshot, SpiralDestinationSettings, UpdateDataSettingsCommand};
use crate::OperationError;
use crate::rollout::manager::RolloutManager;
use crate::sync::MutexRecovery;
use crate::training::manager::TrainingManager;
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;

pub(crate) struct SpiralConnectionController {
    current: Mutex<SpiralConnectionSettings>,
    transition: Mutex<()>,
    stores: SpiralConnectionStores,
    activities: ProjectStateManagers,
}

pub(crate) struct SpiralConnectionStores {
    backend_state: Arc<BackendStateStore>,
    storage: Arc<StorageManager>,
    preferences: Arc<StudioPreferencesStore>,
}

pub(crate) struct ProjectStateManagers {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
    workflow: WorkflowStateManagers,
}

pub(crate) struct WorkflowStateManagers {
    workflow: Arc<WorkflowManager>,
    workflow_agent: Arc<WorkflowAgentManager>,
}

impl SpiralConnectionStores {
    pub(crate) fn new(
        backend_state: Arc<BackendStateStore>,
        storage: Arc<StorageManager>,
        preferences: Arc<StudioPreferencesStore>,
    ) -> Self {
        Self {
            backend_state,
            storage,
            preferences,
        }
    }
}

impl WorkflowStateManagers {
    pub(crate) fn new(
        workflow: Arc<WorkflowManager>,
        workflow_agent: Arc<WorkflowAgentManager>,
    ) -> Self {
        Self {
            workflow,
            workflow_agent,
        }
    }
}

impl ProjectStateManagers {
    pub(crate) fn new(
        training: Arc<TrainingManager>,
        rollout: Arc<RolloutManager>,
        workflow: WorkflowStateManagers,
    ) -> Self {
        Self {
            training,
            rollout,
            workflow,
        }
    }
}

impl SpiralConnectionController {
    pub(crate) fn new(stores: SpiralConnectionStores, activities: ProjectStateManagers) -> Self {
        Self {
            current: Mutex::new(stores.storage.connection_settings()),
            transition: Mutex::new(()),
            stores,
            activities,
        }
    }

    pub(crate) fn settings(&self) -> DataSettingsSnapshot {
        let current = self.current.recover_lock().clone();
        let preferences = self.stores.preferences.get();
        DataSettingsSnapshot::new(
            SpiralDestinationSettings::new(
                current.endpoint(),
                current.spfs_endpoint(),
                current.project_id().unwrap_or_default(),
            )
            .with_client(
                current
                    .credentials()
                    .map(SpiralCredentials::client_id)
                    .unwrap_or_default(),
                current.credentials().is_some(),
            )
            .with_namespace(self.stores.storage.namespace()),
            preferences.training().endpoint(),
            preferences.video().enabled(),
        )
    }

    pub(crate) fn update(
        &self,
        command: UpdateDataSettingsCommand,
        catalog_was_available: bool,
    ) -> Result<DataSettingsSnapshot, OperationError> {
        let _transition = self.transition.recover_lock();
        let previous_connection = self.current.recover_lock().clone();
        let current_credentials = previous_connection.credentials().cloned();
        let credentials = match command.client_secret() {
            Some("") => None,
            Some(secret) => Some(SpiralCredentials::new(
                command.client_id().to_owned(),
                secret.to_owned(),
            )),
            None if command.client_id().is_empty() => None,
            None => current_credentials
                .clone()
                .filter(|credentials| credentials.client_id() == command.client_id())
                .ok_or_else(|| {
                    OperationError::InvalidInput(
                        "Enter the client secret when changing the Spiral client ID".to_owned(),
                    )
                })
                .map(Some)?,
        };
        let next_connection = SpiralConnectionSettings::new(
            command.spiral_endpoint().to_owned(),
            command.spiral_spfs_endpoint().to_owned(),
        )
        .and_then(|settings| settings.with_project(command.project_id()))
        .map(|settings| settings.with_credentials(credentials))
        .map_err(OperationError::InvalidInput)?;
        let connection_changed = next_connection != previous_connection;
        if connection_changed
            && self
                .activities
                .workflow
                .workflow
                .state()
                .snapshot()
                .phase()
                .active()
        {
            return Err(OperationError::Conflict(
                "Cancel the active Workflow run before changing the Spiral destination or credentials."
                    .to_owned(),
            ));
        }

        let previous_preferences = self.stores.preferences.get();
        let staged_preferences = self
            .stores
            .preferences
            .stage_update(
                TrainingRuntimeSettings::new(command.training_endpoint()),
                VideoSettings::new(command.video_enabled()),
            )
            .map_err(OperationError::InvalidInput)?;

        self.stores
            .storage
            .save_studio_preferences(&staged_preferences)
            .map_err(OperationError::External)?;
        if connection_changed {
            if let Err(error) = self
                .stores
                .storage
                .configure_connection(next_connection.clone())
            {
                let _ = self
                    .stores
                    .storage
                    .save_studio_preferences(&previous_preferences);
                return Err(OperationError::InvalidInput(error));
            }
            if let Err(error) = self.stores.backend_state.save_connection(&next_connection) {
                let _ = self
                    .stores
                    .storage
                    .configure_connection(previous_connection.clone());
                let _ = self
                    .stores
                    .storage
                    .save_studio_preferences(&previous_preferences);
                return Err(OperationError::Internal(error));
            }
            *self.current.recover_lock() = next_connection;
        }
        self.stores.preferences.replace(staged_preferences);

        if connection_changed || !catalog_was_available {
            self.reload_project_state()?;
        }
        Ok(self.settings())
    }

    fn reload_project_state(&self) -> Result<(), OperationError> {
        self.activities
            .training
            .reload_from_storage()
            .map_err(OperationError::External)?;
        self.activities
            .rollout
            .reload_from_storage()
            .map_err(OperationError::External)?;
        self.activities
            .workflow
            .workflow
            .reload_from_storage()
            .map_err(OperationError::External)?;
        self.activities
            .workflow
            .workflow_agent
            .reload_from_storage()
            .map_err(OperationError::External)
    }
}
