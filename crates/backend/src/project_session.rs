//! One owner for project lifetime. Published sessions never change destination.

pub(crate) mod bootstrap;
pub(crate) mod context;
pub(crate) mod http;
pub(crate) mod queries;
pub(crate) mod tasks;

use std::collections::BTreeSet;
use std::sync::Arc;

use spiral_robotics_common::project::ProjectSessionInfo;
use spiral_robotics_common::settings::{
    DataSettings, SpiralConnectionUpdate, SpiralSettings, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection as HardwareModeSelection,
};
use spiral_robotics_storage::episodes::RecordingBuffer;
use spiral_robotics_storage::{
    BackendStateStore, ProjectStore, SpiralConnectionSettings, SpiralCredentials, StorageManager,
};
use tokio::sync::{Mutex, watch};

use crate::OperationError;
use crate::curation::manager::CurationManager;
use crate::data::catalog_lease::CatalogMutationLease;
use crate::data::settings::{DataSettingsUpdate, UpdateDataSettingsCommand};
use crate::project_admission::{ProjectActivity, ProjectAdmissionGate, ProjectTransition};
use crate::recording_session::RecordingSessionManager;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::{RolloutHardwareResources, RolloutManager};
use crate::training::manager::TrainingManager;
use crate::training::remote::ConfiguredHttpTrainingBackend;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_common::status::{BackendReadiness, BackendReadinessState};

/// Shared backend lifecycle published to the browser and consulted by
/// catalog-dependent request admission.
#[derive(Clone)]
pub(crate) struct ProjectReadiness {
    current: tokio::sync::watch::Sender<BackendReadiness>,
}

impl ProjectReadiness {
    pub(crate) fn new() -> Self {
        let (current, receiver) = tokio::sync::watch::channel(BackendReadiness::default());
        drop(receiver);
        Self { current }
    }

    pub(crate) fn current(&self) -> BackendReadiness {
        self.current.borrow().clone()
    }

    #[cfg(test)]
    fn subscribe(&self) -> tokio::sync::watch::Receiver<BackendReadiness> {
        self.current.subscribe()
    }

    fn bootstrapping(&self, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Bootstrapping,
            false,
            detail,
        ));
    }

    pub(crate) fn ready(&self, configured: bool) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Ready,
            configured,
            "Studio services are ready",
        ));
    }

    pub(crate) fn failed(&self, catalog_available: bool, error: &OperationError) {
        self.current.send_replace(
            BackendReadiness::new(
                BackendReadinessState::Degraded,
                catalog_available,
                error.message(),
            )
            .with_failure_code(error.code()),
        );
    }

    pub(crate) fn catalog_unavailable(&self) -> Option<OperationError> {
        let current = self.current();
        (!current.catalog_available()).then(|| Self::unavailable_error(&current))
    }

    fn unavailable_error(current: &BackendReadiness) -> OperationError {
        use spiral_robotics_common::command_failure::CommandFailureCode;
        let detail = current.detail().to_owned();
        if current.failure_code() == Some(CommandFailureCode::IncompatibleWorkflow) {
            OperationError::IncompatibleWorkflow(detail)
        } else if current.state() == BackendReadinessState::Bootstrapping {
            OperationError::ProjectTransition(detail)
        } else {
            OperationError::ProjectUnavailable(detail)
        }
    }
}

pub(crate) struct ProjectSession {
    info: ProjectSessionInfo,
    catalog: ProjectCatalog,
    activities: ProjectActivities,
    preferences: Arc<crate::data::preferences::RuntimePreferences>,
    recording: Arc<RecordingSessionManager>,
    readiness: ProjectReadiness,
}

struct ProjectCatalog {
    workspace: Arc<RobotWorkspace>,
    recording_buffer: Arc<RecordingBuffer>,
    storage: Arc<StorageManager>,
    lease: Arc<CatalogMutationLease>,
}

struct ProjectActivities {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
    workflow: Arc<WorkflowManager>,
    curation: Arc<CurationManager>,
}

pub(crate) struct ProjectHardware {
    hardware: Arc<HardwareHandle>,
    cameras: Arc<CameraManager>,
    video_encoder: VideoEncoderSettings,
}

impl ProjectHardware {
    pub(crate) fn new(
        hardware: Arc<HardwareHandle>,
        cameras: Arc<CameraManager>,
        video_encoder: VideoEncoderSettings,
    ) -> Self {
        Self {
            hardware,
            cameras,
            video_encoder,
        }
    }
}

pub(crate) struct ProjectSessionFactory {
    hardware: ProjectHardware,
    table_prefix: String,
    studio: spiral_robotics_storage::StudioStore,
    admission: Arc<ProjectAdmissionGate>,
}

impl ProjectSessionFactory {
    pub(crate) fn new(
        hardware: ProjectHardware,
        table_prefix: String,
        studio: spiral_robotics_storage::StudioStore,
    ) -> Self {
        Self {
            hardware,
            table_prefix,
            studio,
            admission: Arc::new(ProjectAdmissionGate::default()),
        }
    }

    fn create(
        &self,
        connection: SpiralConnectionSettings,
    ) -> Result<Arc<ProjectSession>, OperationError> {
        let info = ProjectSessionInfo::new(
            uuid::Uuid::new_v4().to_string(),
            connection.project_id().unwrap_or_default().to_owned(),
        );
        let database = Arc::new(spiral_robotics_storage::AdbcDatabase::new(
            connection.clone(),
        ));
        let store = Arc::new(
            ProjectStore::new(connection, self.table_prefix.clone(), database)
                .map_err(OperationError::External)?,
        );
        Ok(self.create_session(info, store))
    }

    fn create_session(
        &self,
        info: ProjectSessionInfo,
        store: Arc<ProjectStore>,
    ) -> Arc<ProjectSession> {
        let studio = self.studio.project(&store.connection_settings());
        let workspace = Arc::new(RobotWorkspace::new(studio.clone()));
        let recording_buffer = Arc::new(RecordingBuffer::new());
        let storage = Arc::new(StorageManager::new(store, studio));
        let lease = Arc::new(CatalogMutationLease::default());
        let preferences = Arc::new(crate::data::preferences::RuntimePreferences::new(
            storage.clone(),
        ));
        let training = Arc::new(TrainingManager::new(
            storage.clone(),
            Arc::new(ConfiguredHttpTrainingBackend::new(preferences.clone())),
            self.admission.clone(),
        ));
        let workflow = Arc::new(WorkflowManager::new(storage.clone()));
        let curation = Arc::new(CurationManager::new(storage.clone()));
        let rollout = RolloutManager::new(
            storage.clone(),
            RolloutHardwareResources::new(
                self.hardware.hardware.clone(),
                self.hardware.cameras.clone(),
                workspace.clone(),
            ),
            preferences.clone(),
        );
        let recording = Arc::new(RecordingSessionManager::new(lease.clone()));
        Arc::new(ProjectSession {
            info,
            catalog: ProjectCatalog {
                workspace,
                recording_buffer,
                storage,
                lease,
            },
            activities: ProjectActivities {
                training,
                rollout,
                workflow,
                curation,
            },
            preferences,
            recording,
            readiness: ProjectReadiness::new(),
        })
    }

    async fn prepare(
        &self,
        connection: SpiralConnectionSettings,
        preparation: ProjectPreparation,
    ) -> Result<Arc<ProjectSession>, OperationError> {
        let candidate = self.create(connection)?;
        candidate.prepare(preparation).await?;
        Ok(candidate)
    }
}

/// Resuming a saved destination leaves feature validation to its first use.
/// Explicit selection and connection tests provision and validate the project.
#[derive(Clone, Copy)]
enum ProjectPreparation {
    Resume,
    Validate,
}

impl ProjectSession {
    fn resolve_connection_update(
        update: SpiralConnectionUpdate,
        previous: &SpiralConnectionSettings,
    ) -> Result<SpiralConnectionSettings, OperationError> {
        let (endpoint, spfs, project, client, secret, clear, huggingface_token) =
            update.into_parts();
        let huggingface_token =
            huggingface_token.or_else(|| previous.huggingface_token().map(str::to_owned));
        let endpoint =
            crate::data::settings::HttpEndpoint::parse(&endpoint, "Spiral endpoint")?.into_string();
        let spfs =
            crate::data::settings::HttpEndpoint::parse(&spfs, "SpFS endpoint")?.into_string();
        let client = client.trim();
        let credentials = if clear || client.is_empty() {
            if client.is_empty() && !secret.trim().is_empty() && !clear {
                return Err(OperationError::invalid_input(
                    "Spiral client secret requires a client ID",
                ));
            }
            None
        } else if !secret.trim().is_empty() {
            Some(SpiralCredentials::new(
                client.to_owned(),
                secret.trim().to_owned(),
            ))
        } else {
            Some(
                previous
                    .credentials()
                    .filter(|credentials| credentials.client_id() == client)
                    .cloned()
                    .ok_or_else(|| {
                        OperationError::invalid_input(
                            "Enter the client secret when changing the Spiral client ID",
                        )
                    })?,
            )
        };
        SpiralConnectionSettings::new(endpoint, spfs)
            .and_then(|settings| settings.with_project(project.trim()))
            .map(|settings| {
                settings
                    .with_credentials(credentials)
                    .with_huggingface_token(huggingface_token)
            })
            .map_err(OperationError::InvalidInput)
    }

    pub(crate) fn info(&self) -> &ProjectSessionInfo {
        &self.info
    }
    fn workspace(&self) -> &Arc<RobotWorkspace> {
        &self.catalog.workspace
    }
    fn recording_buffer(&self) -> &Arc<RecordingBuffer> {
        &self.catalog.recording_buffer
    }
    fn storage(&self) -> &Arc<StorageManager> {
        &self.catalog.storage
    }
    fn catalog_lease(&self) -> &Arc<CatalogMutationLease> {
        &self.catalog.lease
    }
    fn preferences(&self) -> &Arc<crate::data::preferences::RuntimePreferences> {
        &self.preferences
    }
    fn recording(&self) -> &Arc<RecordingSessionManager> {
        &self.recording
    }
    fn readiness(&self) -> &ProjectReadiness {
        &self.readiness
    }
    fn training(&self) -> &Arc<TrainingManager> {
        &self.activities.training
    }
    fn rollout(&self) -> &Arc<RolloutManager> {
        &self.activities.rollout
    }
    fn workflow(&self) -> &Arc<WorkflowManager> {
        &self.activities.workflow
    }
    fn curation(&self) -> &Arc<CurationManager> {
        &self.activities.curation
    }

    pub(crate) async fn settings(&self) -> Result<DataSettings, OperationError> {
        let preferences = self
            .preferences
            .load()
            .await
            .map_err(OperationError::External)?;
        Ok(DataSettings::new(
            self.connection_settings(),
            TrainingRuntimeSettings::new(preferences.training().endpoint()),
            VideoSettings::new(preferences.video().enabled()),
        ))
    }

    pub(crate) fn connection_settings(&self) -> SpiralSettings {
        let connection = self.storage().connection_settings();
        SpiralSettings::new(
            connection.endpoint(),
            connection.spfs_endpoint(),
            connection.project_id().unwrap_or_default(),
        )
        .with_client(
            connection
                .credentials()
                .map(SpiralCredentials::client_id)
                .unwrap_or_default(),
            connection.credentials().is_some(),
        )
        .with_huggingface_token_configured(connection.huggingface_token().is_some())
        .with_table_prefix(self.storage().table_prefix())
    }

    async fn prepare(&self, preparation: ProjectPreparation) -> Result<(), OperationError> {
        if matches!(preparation, ProjectPreparation::Validate) && !self.storage().is_configured() {
            return Err(OperationError::invalid_input(
                "Choose a Spiral project in Settings before loading it.",
            ));
        }
        if matches!(preparation, ProjectPreparation::Validate) {
            let (available, detail) = self.storage().test_spiral().await;
            if !available {
                return Err(OperationError::External(detail));
            }
        }
        self.readiness.ready(self.storage().is_configured());
        Ok(())
    }

    fn busy(&self) -> bool {
        self.catalog_lease().is_held()
            || self.recording().episode().is_some()
            || self
                .training()
                .snapshot()
                .runs()
                .iter()
                .any(|run| run.phase().active())
            || self.rollout().is_active()
            || self.workflow().is_active()
    }

    pub(crate) async fn shutdown(&self) {
        self.workflow().shutdown().await;
        self.curation().shutdown().await;
        self.training().shutdown().await;
        self.rollout().shutdown().await;
    }
}

/// The active pointer is published once, after all candidate loads succeed.
/// Requests clone one session and retain it; no getter retargets their storage.
pub(crate) struct ProjectCoordinator {
    active: watch::Sender<Arc<ProjectSession>>,
    transition: Mutex<()>,
    factory: ProjectSessionFactory,
    backend_state: Arc<BackendStateStore>,
}

/// Readiness follows the same lifetime as exclusive project admission. Dropping
/// a failed or canceled change restores the original session after unlocking.
struct PendingProjectChange {
    transition: Option<ProjectTransition>,
    readiness: ProjectReadiness,
    previous: BackendReadiness,
}

impl PendingProjectChange {
    fn into_activity(mut self) -> ProjectActivity {
        // Publication retires the original session; its readiness must not be
        // restored and mistaken for a usable old session by an event subscriber.
        self.transition.take().unwrap().into_activity()
    }
}

impl Drop for PendingProjectChange {
    fn drop(&mut self) {
        if let Some(transition) = self.transition.take() {
            drop(transition);
            self.readiness.current.send_replace(self.previous.clone());
        }
    }
}

impl ProjectCoordinator {
    pub(crate) fn new(
        factory: ProjectSessionFactory,
        backend_state: Arc<BackendStateStore>,
        connection: SpiralConnectionSettings,
    ) -> Result<Arc<Self>, OperationError> {
        let initial = factory.create(connection)?;
        let (active, receiver) = watch::channel(initial);
        drop(receiver);
        Ok(Arc::new(Self {
            active,
            transition: Mutex::new(()),
            factory,
            backend_state,
        }))
    }

    pub(crate) fn active(&self) -> Arc<ProjectSession> {
        self.active.borrow().clone()
    }
    pub(crate) fn backend_state(&self) -> &Arc<BackendStateStore> {
        &self.backend_state
    }
    #[cfg(test)]
    fn subscribe(&self) -> watch::Receiver<Arc<ProjectSession>> {
        self.active.subscribe()
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        &self.factory.hardware.hardware
    }
    fn cameras(&self) -> &Arc<CameraManager> {
        &self.factory.hardware.cameras
    }
    fn video_encoder(&self) -> &VideoEncoderSettings {
        &self.factory.hardware.video_encoder
    }

    fn resolve(&self, session_id: &str) -> Result<Arc<ProjectSession>, OperationError> {
        let current = self.active();
        if current.info().id() != session_id {
            return Err(OperationError::StaleProject(
                "The active project changed. Refresh this view before trying again.".to_owned(),
            ));
        }
        Ok(current)
    }

    fn admit(&self, session_id: &str) -> Result<ProjectActivity, OperationError> {
        let activity = self.factory.admission.admit_activity()?;
        self.resolve(session_id)?;
        Ok(activity)
    }

    fn begin_change(
        &self,
        origin: &ProjectSession,
        detail: &str,
    ) -> Result<PendingProjectChange, OperationError> {
        let transition = self.factory.admission.begin_transition()?;
        let readiness = origin.readiness().clone();
        let previous = readiness.current();
        readiness.bootstrapping(detail);
        Ok(PendingProjectChange {
            transition: Some(transition),
            readiness,
            previous,
        })
    }

    async fn require_idle(
        &self,
        session: &ProjectSession,
        kind: ProjectChangeKind,
    ) -> Result<(), OperationError> {
        let hardware = self.hardware().state();
        if session.busy()
            || hardware.recording().is_some()
            || !matches!(hardware.mode(), HardwareModeSelection::Idle)
        {
            return Err(OperationError::conflict(
                "Stop recording, replay, rollout, training, Workflow, and the Assistant before changing projects.",
            ));
        }
        if matches!(kind, ProjectChangeKind::Reload) {
            return Ok(());
        }
        if matches!(kind, ProjectChangeKind::Import) {
            for (id, connection) in hardware.connections() {
                if connection.connected() {
                    self.hardware()
                        .disconnect(id)
                        .await
                        .map_err(OperationError::External)?;
                }
            }
            return Ok(());
        }
        let visible = session.workspace().try_list().await.ok().map(|robots| {
            robots
                .into_iter()
                .map(|robot| robot.id().to_owned())
                .collect::<BTreeSet<_>>()
        });
        match DestinationChangeContext::new(&hardware, visible.as_ref()).admission() {
            DestinationChangeAdmission::Ready => Ok(()),
            DestinationChangeAdmission::Busy => Err(OperationError::conflict(
                "Disconnect robots before changing projects.",
            )),
            DestinationChangeAdmission::DisconnectOrphaned(ids) => {
                for id in ids {
                    self.hardware()
                        .disconnect(&id)
                        .await
                        .map_err(OperationError::External)?;
                }
                Ok(())
            }
        }
    }

    fn publish(&self, candidate: Arc<ProjectSession>, transition: PendingProjectChange) {
        self.hardware().clear_recording_signals();
        let previous = self.active.send_replace(candidate.clone());
        let _activity = transition.into_activity();
        tokio::spawn(async move {
            previous.shutdown().await;
        });
    }

    pub(crate) async fn update_connection(
        &self,
        origin: &ProjectSession,
        update: SpiralConnectionUpdate,
    ) -> Result<(), OperationError> {
        let _serialization = self.transition.lock().await;
        self.resolve(origin.info().id())?;
        let connection = ProjectSession::resolve_connection_update(
            update,
            &origin.storage().connection_settings(),
        )?;
        let transition = self.begin_change(origin, "Checking the project connection")?;
        self.require_idle(origin, ProjectChangeKind::Destination)
            .await?;
        let candidate = self
            .factory
            .prepare(connection.clone(), ProjectPreparation::Validate)
            .await?;
        self.backend_state
            .save_connection(&connection)
            .await
            .map_err(OperationError::Internal)?;
        self.publish(candidate, transition);
        Ok(())
    }

    pub(crate) async fn update_settings(
        &self,
        origin: &ProjectSession,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsUpdate, OperationError> {
        let command = command.validated()?;
        let _serialization = self.transition.lock().await;
        self.resolve(origin.info().id())?;
        let previous = origin.storage().connection_settings();
        let connection = ProjectSession::resolve_connection_update((&command).into(), &previous)?;
        let preferences = spiral_robotics_storage::StoredStudioPreferences::from_settings(
            command.training_endpoint(),
            command.video_enabled(),
        )
        .map_err(OperationError::InvalidInput)?;
        let replace = connection != previous
            || !origin.readiness().current().catalog_available()
            || origin
                .preferences()
                .load()
                .await
                .map_err(OperationError::External)?
                .training()
                .endpoint()
                != command.training_endpoint();
        if !replace {
            let _activity = self.admit(origin.info().id())?;
            origin
                .storage()
                .save_studio_preferences(&preferences)
                .await
                .map_err(OperationError::External)?;
            origin.preferences().replace(preferences).await;
            return Ok(DataSettingsUpdate::new(origin.settings().await?, false));
        }
        let transition = self.begin_change(origin, "Loading Studio project")?;
        self.require_idle(
            origin,
            if connection == previous {
                ProjectChangeKind::Reload
            } else {
                ProjectChangeKind::Destination
            },
        )
        .await?;
        let candidate = self
            .factory
            .prepare(connection.clone(), ProjectPreparation::Validate)
            .await?;
        candidate
            .storage()
            .save_studio_preferences(&preferences)
            .await
            .map_err(OperationError::External)?;
        candidate.preferences().replace(preferences).await;
        candidate.training().refresh_readiness().await;
        if connection != previous {
            self.backend_state
                .save_connection(&connection)
                .await
                .map_err(OperationError::Internal)?;
        }
        let result = candidate.settings().await?;
        self.publish(candidate, transition);
        Ok(DataSettingsUpdate::new(result, true))
    }

    async fn resume(&self, origin: &ProjectSession) -> Result<(), OperationError> {
        self.reload(origin, ProjectPreparation::Resume).await
    }

    pub(crate) async fn recover(&self, origin: &ProjectSession) -> Result<(), OperationError> {
        self.reload(origin, ProjectPreparation::Validate).await
    }

    async fn reload(
        &self,
        origin: &ProjectSession,
        preparation: ProjectPreparation,
    ) -> Result<(), OperationError> {
        let _serialization = self.transition.lock().await;
        self.resolve(origin.info().id())?;
        let transition = self.begin_change(origin, "Loading Studio project")?;
        self.require_idle(origin, ProjectChangeKind::Reload).await?;
        let candidate = self
            .factory
            .prepare(origin.storage().connection_settings(), preparation)
            .await?;
        self.publish(candidate, transition);
        Ok(())
    }

    pub(crate) async fn import(
        &self,
        origin: &ProjectSession,
        document: &[u8],
    ) -> Result<spiral_robotics_common::datasets::ProjectImportSummary, OperationError> {
        let _serialization = self.transition.lock().await;
        self.resolve(origin.info().id())?;
        let transition = self.begin_change(origin, "Importing and loading the project")?;
        self.require_idle(origin, ProjectChangeKind::Import).await?;
        // Import uses a separate store at the same immutable destination. Its
        // importer coordinates the Spiral data and local SQLite transactions.
        let candidate = self
            .factory
            .create(origin.storage().connection_settings())?;
        let summary = candidate
            .storage()
            .import_project_bundle(document)
            .await
            .map_err(OperationError::External)?;
        candidate
            .readiness()
            .ready(candidate.storage().is_configured());
        self.publish(candidate, transition);
        Ok(summary)
    }
}

enum ProjectChangeKind {
    Destination,
    Reload,
    Import,
}

#[derive(Debug, Eq, PartialEq)]
enum DestinationChangeAdmission {
    Ready,
    DisconnectOrphaned(Vec<String>),
    Busy,
}

struct DestinationChangeContext {
    hardware_busy: bool,
    connected_robot_ids: Vec<String>,
    current_robot_ids: Option<BTreeSet<String>>,
}

impl DestinationChangeContext {
    fn new(hardware: &HardwareState, current_robot_ids: Option<&BTreeSet<String>>) -> Self {
        Self {
            hardware_busy: hardware.recording().is_some()
                || !matches!(hardware.mode(), HardwareModeSelection::Idle),
            connected_robot_ids: hardware
                .connections()
                .iter()
                .filter(|(_, connection)| connection.connected())
                .map(|(id, _)| id.clone())
                .collect(),
            current_robot_ids: current_robot_ids.cloned(),
        }
    }

    #[cfg(test)]
    const fn with_active_work(mut self, active: bool) -> Self {
        self.hardware_busy |= active;
        self
    }

    fn admission(self) -> DestinationChangeAdmission {
        if self.hardware_busy {
            return DestinationChangeAdmission::Busy;
        }
        if self.connected_robot_ids.is_empty() {
            return DestinationChangeAdmission::Ready;
        }
        match self.current_robot_ids {
            Some(ids) if self.connected_robot_ids.iter().any(|id| ids.contains(id)) => {
                DestinationChangeAdmission::Busy
            }
            Some(ids) => DestinationChangeAdmission::DisconnectOrphaned(
                self.connected_robot_ids
                    .into_iter()
                    .filter(|id| !ids.contains(id))
                    .collect(),
            ),
            None => DestinationChangeAdmission::DisconnectOrphaned(self.connected_robot_ids),
        }
    }

    #[cfg(test)]
    fn idle_with_connection(id: &str) -> Self {
        Self {
            hardware_busy: false,
            connected_robot_ids: vec![id.to_owned()],
            current_robot_ids: None,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct ProjectFixture {
        pub(crate) projects: Arc<ProjectCoordinator>,
        _directory: tempfile::TempDir,
    }

    impl ProjectFixture {
        pub(crate) fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let hardware = ProjectHardware::new(
                Arc::new(HardwareHandle::spawn(Default::default())),
                Arc::new(CameraManager::new()),
                VideoEncoderSettings::new("ffmpeg".to_owned(), "libx264".to_owned()),
            );
            let factory = ProjectSessionFactory::new(
                hardware,
                "project_lifetime_test".to_owned(),
                spiral_robotics_storage::StudioStore::in_memory(),
            );
            let backend_state = Arc::new(BackendStateStore::new(
                spiral_robotics_storage::StudioStore::at(directory.path().join("studio.sqlite3")),
                directory.path().join("backend.json"),
            ));
            let connection = SpiralConnectionSettings::new(
                "https://api.spiraldb.dev".to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap();
            Self {
                projects: ProjectCoordinator::new(factory, backend_state, connection).unwrap(),
                _directory: directory,
            }
        }

        pub(crate) fn replace(&self, project_id: &str) -> Arc<ProjectSession> {
            let connection = self
                .projects
                .active()
                .storage()
                .connection_settings()
                .with_project(project_id)
                .unwrap();
            let candidate = self.projects.factory.create(connection).unwrap();
            let transition = self
                .projects
                .begin_change(&self.projects.active(), "Loading the project")
                .unwrap();
            self.projects.publish(candidate.clone(), transition);
            candidate
        }
    }

    #[test]
    fn huggingface_settings_preserve_replace_and_remove_the_private_token() {
        let previous = SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".into(),
            "https://spfs.spiraldb.dev".into(),
        )
        .unwrap()
        .with_project("project")
        .unwrap()
        .with_huggingface_token(Some("hf_saved".into()));
        for (change, expected) in [
            (None, Some("hf_saved")),
            (Some(" hf_replacement ".to_owned()), Some("hf_replacement")),
            (Some(String::new()), None),
        ] {
            let update = SpiralConnectionUpdate::new(
                previous.endpoint(),
                previous.spfs_endpoint(),
                "project",
            )
            .with_huggingface_token(change);
            let decoded: SpiralConnectionUpdate =
                serde_json::from_str(&serde_json::to_string(&update).unwrap()).unwrap();
            let resolved = ProjectSession::resolve_connection_update(decoded, &previous).unwrap();
            assert_eq!(resolved.huggingface_token(), expected);
        }
    }

    #[test]
    fn connection_repairs_preserve_credentials_unless_the_edit_explicitly_replaces_them() {
        let previous = SpiralConnectionSettings::new(
            "https://api.spiraldb.dev".into(),
            "https://spfs.spiraldb.dev".into(),
        )
        .unwrap()
        .with_project("old")
        .unwrap()
        .with_credentials(Some(SpiralCredentials::new(
            "client".into(),
            "saved-secret".into(),
        )));
        let update = SpiralConnectionUpdate::new(
            "https://api.spiraldb.dev",
            "https://spfs.spiraldb.dev",
            "new",
        )
        .with_client_credentials("client", "");
        let repaired =
            ProjectSession::resolve_connection_update(update.clone(), &previous).unwrap();
        assert_eq!(repaired.project_id(), Some("new"));
        assert!(repaired.credentials() == previous.credentials());
        assert!(
            ProjectSession::resolve_connection_update(
                update.clone().with_client_credentials("different", ""),
                &previous,
            )
            .is_err()
        );
        assert!(
            ProjectSession::resolve_connection_update(
                update.with_clear_client_secret(true),
                &previous,
            )
            .unwrap()
            .credentials()
            .is_none()
        );
    }

    #[tokio::test]
    async fn settings_present_the_session_destination_and_preferences_without_secrets() {
        let fixture = ProjectFixture::new();
        let connection = fixture
            .projects
            .active()
            .storage()
            .connection_settings()
            .with_project("configured_project")
            .unwrap()
            .with_credentials(Some(SpiralCredentials::new(
                "client".to_owned(),
                "private-secret".to_owned(),
            )));
        let session = fixture.projects.factory.create(connection).unwrap();
        let preferences = spiral_robotics_storage::StoredStudioPreferences::from_settings(
            "http://127.0.0.1:5050",
            false,
        )
        .unwrap();
        session.preferences().replace(preferences).await;
        let settings = session.settings().await.unwrap();
        assert_eq!(settings.spiral().project_id(), "configured_project");
        assert_eq!(settings.spiral().endpoint(), "https://api.spiraldb.dev");
        assert_eq!(
            settings.spiral().spfs_endpoint(),
            "https://spfs.spiraldb.dev"
        );
        assert_eq!(settings.spiral().table_prefix(), "project_lifetime_test");
        assert_eq!(settings.spiral().client_id(), "client");
        assert!(settings.spiral().client_secret_configured());
        assert_eq!(settings.training().endpoint(), "http://127.0.0.1:5050");
        assert!(!settings.video().enabled());
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("private-secret")
        );
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_preparation_keeps_the_published_session_and_releases_admission() {
        let fixture = ProjectFixture::new();
        let original = fixture.projects.active();
        original.readiness().failed(
            true,
            &OperationError::External("History recovery needs attention".to_owned()),
        );
        let previous_readiness = original.readiness().current();
        let announcements = fixture.projects.subscribe();
        // No configured project: the real storage preparation fails locally,
        // before contacting Spiral, after acquiring transition ownership.
        assert!(fixture.projects.recover(&original).await.is_err());
        assert!(Arc::ptr_eq(&fixture.projects.active(), &original));
        assert!(!announcements.has_changed().unwrap());
        assert!(fixture.projects.admit(original.info().id()).is_ok());
        assert_eq!(original.readiness().current(), previous_readiness);
        assert!(fixture.projects.backend_state.project_id().is_none());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn canceled_change_restores_readiness_and_releases_admission() {
        let fixture = ProjectFixture::new();
        let original = fixture.projects.active();
        original
            .readiness()
            .ready(original.storage().is_configured());
        let previous_readiness = original.readiness().current();
        let mut readiness = original.readiness().subscribe();
        let projects = fixture.projects.clone();
        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let change = tokio::spawn(async move {
            let session = projects.active();
            let _change = projects
                .begin_change(&session, "Loading another project")
                .unwrap();
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_receiver.await.unwrap();
        readiness.changed().await.unwrap();
        let loading = readiness.borrow_and_update().clone();
        assert_eq!(loading.state(), BackendReadinessState::Bootstrapping);
        assert_eq!(loading.detail(), "Loading another project");
        assert!(!loading.catalog_available());
        assert!(fixture.projects.admit(original.info().id()).is_err());

        change.abort();
        assert!(change.await.unwrap_err().is_cancelled());
        readiness.changed().await.unwrap();
        assert_eq!(*readiness.borrow_and_update(), previous_readiness);
        assert!(Arc::ptr_eq(&fixture.projects.active(), &original));
        assert!(fixture.projects.admit(original.info().id()).is_ok());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn publication_rejects_stale_requests_without_retargeting_old_storage() {
        let fixture = ProjectFixture::new();
        let original = fixture.projects.active();
        original
            .readiness()
            .ready(original.storage().is_configured());
        let old_connection = original.storage().connection_settings();
        let replacement = fixture.replace("project_b");
        assert!(!Arc::ptr_eq(&original, &replacement));
        assert_ne!(original.info().id(), replacement.info().id());
        assert_eq!(
            original.readiness().current().state(),
            BackendReadinessState::Bootstrapping
        );
        assert!(original.storage().connection_settings() == old_connection);
        assert_eq!(
            replacement.storage().connection_settings().project_id(),
            Some("project_b")
        );
        assert!(fixture.projects.resolve(original.info().id()).is_err());
        assert!(fixture.projects.admit(original.info().id()).is_err());
        assert!(fixture.projects.admit(replacement.info().id()).is_ok());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn admitted_request_blocks_replacement_until_its_last_owner_finishes() {
        let fixture = ProjectFixture::new();
        let original = fixture.projects.active();
        original
            .readiness()
            .ready(original.storage().is_configured());
        let readiness = original.readiness().subscribe();
        let request = fixture.projects.admit(original.info().id()).unwrap();
        let background = request.clone();
        drop(request);
        assert!(matches!(
            fixture.projects.recover(&original).await,
            Err(OperationError::Conflict(_))
        ));
        assert!(Arc::ptr_eq(&fixture.projects.active(), &original));
        assert!(
            !readiness.has_changed().unwrap(),
            "rejected admission must not pause the project"
        );
        drop(background);
        let replacement = fixture.replace("project_b");
        assert_eq!(fixture.projects.active().info(), replacement.info());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[test]
    fn unavailable_catalog_can_detach_idle_hidden_robots_during_cutover() {
        let hardware = DestinationChangeContext::idle_with_connection("leader");
        assert_eq!(
            hardware.admission(),
            DestinationChangeAdmission::DisconnectOrphaned(vec!["leader".to_owned()]),
        );
    }

    #[test]
    fn visible_connected_robot_still_blocks_destination_cutover() {
        let mut hardware = DestinationChangeContext::idle_with_connection("leader");
        hardware.current_robot_ids = Some(BTreeSet::from(["leader".to_owned()]));
        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy);
    }

    #[test]
    fn active_work_never_uses_the_hidden_robot_recovery_path() {
        let hardware =
            DestinationChangeContext::idle_with_connection("leader").with_active_work(true);
        assert_eq!(hardware.admission(), DestinationChangeAdmission::Busy);
    }
}

#[cfg(test)]
mod loading_tests;
