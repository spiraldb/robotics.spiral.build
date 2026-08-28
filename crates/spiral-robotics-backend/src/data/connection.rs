//! Authoritative Spiral project-session configuration and transitions.

use std::sync::{Arc, Mutex};

use spiral_robotics_storage::{
    BackendStateStore, SpiralConnectionSettings, SpiralCredentials, StorageManager,
    StudioPreferencesStore,
};

use super::{
    DataSettingsSnapshot, DataSettingsUpdate, SpiralDestinationSettings, UpdateDataSettingsCommand,
};
use crate::OperationError;
use crate::rollout::manager::RolloutManager;
use crate::sync::MutexRecovery;
use crate::training::manager::TrainingManager;
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;

pub(crate) struct SpiralConnectionController {
    current: Mutex<SpiralConnectionSettings>,
    transition: tokio::sync::Mutex<()>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectStateReload {
    Required,
    Opportunistic,
    Unnecessary,
}

impl ProjectStateReload {
    fn for_settings_update(connection_changed: bool, catalog_was_available: bool) -> Self {
        if connection_changed {
            Self::Required
        } else if catalog_was_available {
            Self::Unnecessary
        } else {
            Self::Opportunistic
        }
    }

    fn resolve(self, reload: Result<(), OperationError>) -> Result<bool, OperationError> {
        match (self, reload) {
            (Self::Required, Ok(())) | (Self::Opportunistic, Ok(())) => Ok(true),
            (Self::Required, Err(error)) => Err(error),
            (Self::Opportunistic, Err(error)) => {
                tracing::warn!(
                    "settings were saved while project-state recovery remained unavailable: {error}"
                );
                Ok(false)
            }
            (Self::Unnecessary, _) => Ok(false),
        }
    }
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
            transition: tokio::sync::Mutex::new(()),
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

    pub(crate) async fn update(
        &self,
        command: UpdateDataSettingsCommand,
        catalog_was_available: bool,
    ) -> Result<DataSettingsUpdate, OperationError> {
        let _transition = self.transition.lock().await;
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

        let staged_preferences = self
            .stores
            .preferences
            .stage_update(command.training_endpoint(), command.video_enabled())
            .map_err(OperationError::InvalidInput)?;

        if connection_changed {
            // A recovery switch must not depend on the old project remaining
            // reachable. Drain and install the new scoped connection before
            // writing project-owned preferences through it.
            self.stores
                .storage
                .configure_connection(next_connection.clone())
                .await
                .map_err(OperationError::InvalidInput)?;
            if let Err(error) = self
                .stores
                .storage
                .save_studio_preferences(&staged_preferences)
                .await
            {
                let _ = self
                    .stores
                    .storage
                    .configure_connection(previous_connection.clone())
                    .await;
                return Err(OperationError::External(error));
            }
            if let Err(error) = self.stores.backend_state.save_connection(&next_connection) {
                let _ = self
                    .stores
                    .storage
                    .configure_connection(previous_connection)
                    .await;
                return Err(OperationError::Internal(error));
            }
            *self.current.recover_lock() = next_connection;
        } else {
            self.stores
                .storage
                .save_studio_preferences(&staged_preferences)
                .await
                .map_err(OperationError::External)?;
        }
        self.stores.preferences.replace(staged_preferences);

        let reload =
            ProjectStateReload::for_settings_update(connection_changed, catalog_was_available);
        let project_state_rehydrated = match reload {
            ProjectStateReload::Unnecessary => false,
            reload => reload.resolve(self.reload_project_state().await)?,
        };
        Ok(DataSettingsUpdate::new(
            self.settings(),
            project_state_rehydrated,
        ))
    }

    pub(crate) async fn recover_project_state(&self) -> Result<(), OperationError> {
        let _transition = self.transition.lock().await;
        self.reload_project_state().await
    }

    async fn reload_project_state(&self) -> Result<(), OperationError> {
        let preferences = self
            .stores
            .storage
            .load_studio_preferences()
            .await
            .map_err(OperationError::External)?;
        self.stores.preferences.replace(preferences);
        self.activities
            .training
            .reload_from_storage()
            .await
            .map_err(OperationError::External)?;
        self.activities
            .rollout
            .reload_from_storage()
            .await
            .map_err(OperationError::External)?;
        self.activities
            .workflow
            .workflow
            .reload_from_storage()
            .await
            .map_err(OperationError::External)?;
        self.activities
            .workflow
            .workflow_agent
            .reload_from_storage()
            .await
            .map_err(OperationError::External)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_changes_keep_project_reload_failures_hard() {
        let failure = OperationError::External("rollout relation unavailable".to_owned());
        assert_eq!(
            ProjectStateReload::for_settings_update(true, false).resolve(Ok(())),
            Ok(true)
        );
        assert_eq!(
            ProjectStateReload::for_settings_update(true, false).resolve(Err(failure.clone())),
            Err(failure)
        );
    }

    #[test]
    fn degraded_unchanged_connections_keep_saved_settings_when_reload_fails() {
        assert_eq!(
            ProjectStateReload::for_settings_update(false, false).resolve(Err(
                OperationError::External("rollout relation unavailable".to_owned())
            )),
            Ok(false)
        );
        assert_eq!(
            ProjectStateReload::for_settings_update(false, false).resolve(Ok(())),
            Ok(true)
        );
    }
}
