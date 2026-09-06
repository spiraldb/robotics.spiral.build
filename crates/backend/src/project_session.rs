//! One owner for project lifetime. Published sessions never change destination.

use std::collections::BTreeSet;
use std::sync::Arc;

use spiral_robotics_common::project::ProjectSessionInfo;
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection as HardwareModeSelection,
};
use spiral_robotics_storage::episodes::EpisodeStore;
use spiral_robotics_storage::{
    BackendStateStore, ProjectStore, SpiralConnectionSettings, SpiralCredentials, StorageManager,
    StudioPreferencesStore,
};
use tokio::sync::{Mutex, watch};

use crate::OperationError;
use crate::data::catalog_lease::CatalogMutationLease;
use crate::data::{
    DataSettingsSnapshot, DataSettingsUpdate, SpiralDestinationSettings, UpdateDataSettingsCommand,
};
use crate::project_admission::{ProjectActivity, ProjectAdmissionGate, ProjectTransition};
use crate::recording_session::RecordingSessionManager;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::{RolloutHardwareResources, RolloutManager};
use crate::training::manager::TrainingManager;
use crate::training::remote::ConfiguredHttpTrainingBackend;
use crate::workflow::agent::WorkflowAgentManager;
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

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<BackendReadiness> {
        self.current.subscribe()
    }

    pub(crate) fn bootstrapping(&self, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Bootstrapping,
            false,
            detail,
        ));
    }

    pub(crate) fn ready(&self) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Ready,
            true,
            "Studio services are ready",
        ));
    }

    pub(crate) fn degraded(&self, catalog_available: bool, detail: impl Into<String>) {
        self.current.send_replace(BackendReadiness::new(
            BackendReadinessState::Degraded,
            catalog_available,
            detail,
        ));
    }

    pub(crate) fn catalog_unavailable(&self) -> Option<String> {
        let current = self.current();
        (!current.catalog_available()).then(|| current.detail().to_owned())
    }

    pub(crate) fn storage_unavailable(&self) -> Option<String> {
        let current = self.current();
        (!current.storage_available()).then(|| current.detail().to_owned())
    }
}

pub(crate) struct ProjectSession {
    info: ProjectSessionInfo,
    catalog: ProjectCatalog,
    activities: ProjectActivities,
    preferences: Arc<StudioPreferencesStore>,
    recording: Arc<RecordingSessionManager>,
    readiness: ProjectReadiness,
}

struct ProjectCatalog {
    workspace: Arc<RobotWorkspace>,
    episodes: Arc<EpisodeStore>,
    storage: Arc<StorageManager>,
    lease: Arc<CatalogMutationLease>,
}

struct ProjectActivities {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
    workflow: Arc<WorkflowManager>,
    assistant: Arc<WorkflowAgentManager>,
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
    namespace: String,
    admission: Arc<ProjectAdmissionGate>,
}

impl ProjectSessionFactory {
    pub(crate) fn new(hardware: ProjectHardware, namespace: String) -> Self {
        Self {
            hardware,
            namespace,
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
        let store = Arc::new(
            ProjectStore::new(connection, self.namespace.clone())
                .map_err(OperationError::External)?,
        );
        let workspace = Arc::new(RobotWorkspace::new(store.clone()));
        let episodes = Arc::new(EpisodeStore::new(store.clone()));
        let storage = Arc::new(StorageManager::new(store));
        let lease = Arc::new(CatalogMutationLease::default());
        let preferences = Arc::new(StudioPreferencesStore::new(Default::default()));
        let training = Arc::new(TrainingManager::new(
            storage.clone(),
            Arc::new(ConfiguredHttpTrainingBackend::new(preferences.clone())),
            self.admission.clone(),
        ));
        let workflow = Arc::new(WorkflowManager::new(storage.clone()));
        let assistant = Arc::new(WorkflowAgentManager::new(
            storage.clone(),
            workflow.clone(),
            lease.clone(),
        ));
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
        Ok(Arc::new(ProjectSession {
            info,
            catalog: ProjectCatalog {
                workspace,
                episodes,
                storage,
                lease,
            },
            activities: ProjectActivities {
                training,
                rollout,
                workflow,
                assistant,
            },
            preferences,
            recording,
            readiness: ProjectReadiness::new(),
        }))
    }

    async fn prepare(
        &self,
        connection: SpiralConnectionSettings,
    ) -> Result<Arc<ProjectSession>, OperationError> {
        let candidate = self.create(connection)?;
        let (available, detail) = candidate.storage().test_spiral().await;
        if !available {
            return Err(OperationError::External(detail));
        }
        candidate.load().await?;
        Ok(candidate)
    }
}

impl ProjectSession {
    pub(crate) fn info(&self) -> &ProjectSessionInfo {
        &self.info
    }
    pub(crate) fn workspace(&self) -> &Arc<RobotWorkspace> {
        &self.catalog.workspace
    }
    pub(crate) fn episodes(&self) -> &Arc<EpisodeStore> {
        &self.catalog.episodes
    }
    pub(crate) fn storage(&self) -> &Arc<StorageManager> {
        &self.catalog.storage
    }
    pub(crate) fn catalog_lease(&self) -> &Arc<CatalogMutationLease> {
        &self.catalog.lease
    }
    pub(crate) fn preferences(&self) -> &Arc<StudioPreferencesStore> {
        &self.preferences
    }
    pub(crate) fn recording(&self) -> &Arc<RecordingSessionManager> {
        &self.recording
    }
    pub(crate) fn readiness(&self) -> &ProjectReadiness {
        &self.readiness
    }
    pub(crate) fn training(&self) -> &Arc<TrainingManager> {
        &self.activities.training
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        &self.activities.rollout
    }
    pub(crate) fn workflow(&self) -> &Arc<WorkflowManager> {
        &self.activities.workflow
    }
    pub(crate) fn workflow_agent(&self) -> &Arc<WorkflowAgentManager> {
        &self.activities.assistant
    }

    pub(crate) fn settings(&self) -> DataSettingsSnapshot {
        let connection = self.storage().connection_settings();
        let preferences = self.preferences.get();
        DataSettingsSnapshot::new(
            SpiralDestinationSettings::new(
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
            .with_namespace(self.storage().namespace()),
            preferences.training().endpoint(),
            preferences.video().enabled(),
        )
    }

    async fn load(&self) -> Result<(), OperationError> {
        let preferences = self
            .storage()
            .load_studio_preferences()
            .await
            .map_err(OperationError::External)?;
        self.preferences.replace(preferences);
        // Preparation only reads and installs private in-memory state. Jobs and
        // hardware effects begin after this entire session has been published.
        tokio::try_join!(
            self.training().prepare_from_storage(),
            self.workflow().prepare_from_storage(),
            self.workflow_agent().prepare_from_storage(),
            self.rollout().prepare_from_storage(),
            async {
                self.workspace()
                    .try_list()
                    .await
                    .map_err(|error| error.to_string())
            },
            self.storage().relation_catalog(),
        )
        .map_err(OperationError::External)?;
        self.training().refresh_readiness().await;
        self.readiness.ready();
        Ok(())
    }

    fn activate(self: &Arc<Self>, activity: ProjectActivity) {
        self.rollout().activate();
        self.training().resume_prepared_runs(activity.clone());
        let session = self.clone();
        tokio::spawn(async move {
            // Recovery writes belong to the published session and finish before
            // another replacement can begin. Each manager serializes this save
            // with commands so it cannot overwrite a newer edit.
            let _activity = activity;
            let (workflow, assistant) = tokio::join!(
                session.workflow().persist_recovery(),
                session.workflow_agent().persist_recovery()
            );
            if let Err(error) = workflow.and(assistant) {
                session.readiness().degraded(true, format!("The project loaded, but recovered history could not be saved: {error}. Test the connection to retry."));
            }
        });
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
            || self.workflow_agent().is_active()
    }

    pub(crate) async fn shutdown(&self) {
        self.workflow_agent().shutdown().await;
        self.workflow().shutdown().await;
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
    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<ProjectSession>> {
        self.active.subscribe()
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        &self.factory.hardware.hardware
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        &self.factory.hardware.cameras
    }
    pub(crate) fn video_encoder(&self) -> &VideoEncoderSettings {
        &self.factory.hardware.video_encoder
    }

    pub(crate) fn resolve(&self, session_id: &str) -> Result<Arc<ProjectSession>, OperationError> {
        let current = self.active();
        if current.info().id() != session_id {
            return Err(OperationError::conflict(
                "The active project changed. Refresh this view before trying again.",
            ));
        }
        Ok(current)
    }

    pub(crate) fn admit(&self, session_id: &str) -> Result<ProjectActivity, OperationError> {
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
        let previous = self.active.send_replace(candidate.clone());
        candidate.activate(transition.into_activity());
        tokio::spawn(async move {
            previous.shutdown().await;
        });
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
        let credentials = match command.client_secret() {
            Some("") => None,
            Some(secret) => Some(SpiralCredentials::new(
                command.client_id().to_owned(),
                secret.to_owned(),
            )),
            None if command.client_id().is_empty() => None,
            None => Some(
                previous
                    .credentials()
                    .filter(|credentials| credentials.client_id() == command.client_id())
                    .cloned()
                    .ok_or_else(|| {
                        OperationError::invalid_input(
                            "Enter the client secret when changing the Spiral client ID",
                        )
                    })?,
            ),
        };
        let connection = SpiralConnectionSettings::new(
            command.spiral_endpoint().to_owned(),
            command.spiral_spfs_endpoint().to_owned(),
        )
        .and_then(|settings| settings.with_project(command.project_id()))
        .map(|settings| settings.with_credentials(credentials))
        .map_err(OperationError::InvalidInput)?;
        let preferences = origin
            .preferences()
            .stage_update(command.training_endpoint(), command.video_enabled())
            .map_err(OperationError::InvalidInput)?;
        let replace = connection != previous
            || origin.preferences().get().training().endpoint() != command.training_endpoint()
            || !origin.readiness().current().catalog_available();
        if !replace {
            let _activity = self.admit(origin.info().id())?;
            origin
                .storage()
                .save_studio_preferences(&preferences)
                .await
                .map_err(OperationError::External)?;
            origin.preferences().replace(preferences);
            return Ok(DataSettingsUpdate::new(origin.settings(), false));
        }
        let transition =
            self.begin_change(origin, "Connecting to SpiralDB and loading the project")?;
        self.require_idle(
            origin,
            if connection == previous {
                ProjectChangeKind::Reload
            } else {
                ProjectChangeKind::Destination
            },
        )
        .await?;
        if connection.project_id() == previous.project_id() {
            origin
                .rollout()
                .flush_persistence()
                .await
                .map_err(OperationError::External)?;
        }
        let candidate = self.factory.prepare(connection.clone()).await?;
        candidate
            .storage()
            .save_studio_preferences(&preferences)
            .await
            .map_err(OperationError::External)?;
        candidate.preferences().replace(preferences);
        candidate.training().refresh_readiness().await;
        if connection != previous {
            self.backend_state
                .save_connection(&connection)
                .map_err(OperationError::Internal)?;
        }
        let result = candidate.settings();
        self.publish(candidate, transition);
        Ok(DataSettingsUpdate::new(result, true))
    }

    pub(crate) async fn recover(&self, origin: &ProjectSession) -> Result<(), OperationError> {
        let _serialization = self.transition.lock().await;
        self.resolve(origin.info().id())?;
        let transition =
            self.begin_change(origin, "Connecting to SpiralDB and loading the project")?;
        self.require_idle(origin, ProjectChangeKind::Reload).await?;
        origin
            .rollout()
            .flush_persistence()
            .await
            .map_err(OperationError::External)?;
        let candidate = self
            .factory
            .prepare(origin.storage().connection_settings())
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
        // database transaction is owned by the existing native bundle importer.
        origin
            .rollout()
            .flush_persistence()
            .await
            .map_err(OperationError::External)?;
        let candidate = self
            .factory
            .create(origin.storage().connection_settings())?;
        // Construct the empty fallback before committing: after commit, failure
        // to load must invalidate the old scope even if no state can be shown.
        let unavailable = self
            .factory
            .create(origin.storage().connection_settings())?;
        let summary = candidate
            .storage()
            .import_project_bundle(document)
            .await
            .map_err(OperationError::External)?;
        if let Err(error) = candidate.load().await {
            let message = format!(
                "The project import committed, but the imported state could not be loaded: {error}. Test the connection to retry loading."
            );
            unavailable.readiness().degraded(false, message.clone());
            self.publish(unavailable, transition);
            return Err(OperationError::External(message));
        }
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
            let factory = ProjectSessionFactory::new(hardware, "project_lifetime_test".to_owned());
            let backend_state = Arc::new(
                BackendStateStore::open_for_test(directory.path().join("backend.json")).unwrap(),
            );
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

    #[tokio::test]
    async fn failed_preparation_keeps_the_published_session_and_releases_admission() {
        let fixture = ProjectFixture::new();
        let original = fixture.projects.active();
        original
            .readiness()
            .degraded(true, "History recovery needs attention");
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
        original.readiness().ready();
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
        assert!(!loading.storage_available());
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
        original.readiness().ready();
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
        original.readiness().ready();
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
