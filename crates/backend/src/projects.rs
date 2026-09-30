//! Every project this process has opened, each keyed by the connection that
//! reaches it. Requests name their project; nothing here is "current".

pub(crate) mod bootstrap;
pub(crate) mod context;
pub(crate) mod http;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::settings::{
    DataSettings, SettingsUpdate, SpiralConnectionUpdate, SpiralSettings, TrainingRuntimeSettings,
    VideoSettings,
};
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection as HardwareModeSelection,
};
use spiral_robotics_storage::episodes::RecordingBuffer;
use spiral_robotics_storage::{
    BackendStateStore, ProjectStore, ProviderKeys, SpiralAccess, SpiralConnection,
    SpiralConnectionSettings, StorageManager, StoredStudioPreferences,
};

use crate::OperationError;
use crate::curation::manager::CurationManager;
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::{RolloutHardwareResources, RolloutManager};
use crate::sync::MutexRecovery;
use crate::training::manager::TrainingManager;

/// The storage and running work of one project. Its connection never changes.
pub(crate) struct OpenProject {
    catalog: ProjectCatalog,
    activities: ProjectActivities,
    recording: Arc<RecordingSession>,
}

struct ProjectCatalog {
    workspace: Arc<RobotWorkspace>,
    recording_buffer: Arc<RecordingBuffer>,
    storage: Arc<StorageManager>,
}

struct ProjectActivities {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
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

/// How every project reaches its data: Studio's local store, the signed-in
/// user's Spiral access, and the provider keys the driver works with.
pub(crate) struct ProjectDataAccess {
    studio: spiral_robotics_storage::StudioStore,
    spiral: SpiralAccess,
    keys: tokio::sync::watch::Receiver<ProviderKeys>,
}

impl ProjectDataAccess {
    pub(crate) fn new(
        studio: spiral_robotics_storage::StudioStore,
        spiral: SpiralAccess,
        keys: tokio::sync::watch::Receiver<ProviderKeys>,
    ) -> Self {
        Self {
            studio,
            spiral,
            keys,
        }
    }
}

pub(crate) struct ProjectFactory {
    hardware: ProjectHardware,
    data: ProjectDataAccess,
}

impl ProjectFactory {
    pub(crate) fn new(hardware: ProjectHardware, data: ProjectDataAccess) -> Self {
        Self { hardware, data }
    }

    fn create(
        &self,
        settings: SpiralConnectionSettings,
    ) -> Result<Arc<OpenProject>, OperationError> {
        let connection = SpiralConnection::new(settings, self.data.spiral.clone());
        let database = Arc::new(
            spiral_robotics_storage::AdbcDatabase::new(connection.clone())
                .with_provider_keys(self.data.keys.clone()),
        );
        let store =
            Arc::new(ProjectStore::new(connection, database).map_err(OperationError::External)?);
        Ok(self.create_from_store(store))
    }

    fn create_from_store(&self, store: Arc<ProjectStore>) -> Arc<OpenProject> {
        let studio = self.data.studio.project(&store.connection_settings());
        let workspace = Arc::new(RobotWorkspace::new(studio.clone()));
        let recording_buffer = Arc::new(RecordingBuffer::new());
        let storage = Arc::new(StorageManager::new(store, studio));
        let training = Arc::new(TrainingManager::new(storage.clone()));
        let curation = Arc::new(CurationManager::new(
            storage.clone(),
            self.data.keys.clone(),
        ));
        let rollout = RolloutManager::new(
            storage.clone(),
            RolloutHardwareResources::new(
                self.hardware.hardware.clone(),
                self.hardware.cameras.clone(),
                workspace.clone(),
            ),
        );
        Arc::new(OpenProject {
            catalog: ProjectCatalog {
                workspace,
                recording_buffer,
                storage,
            },
            activities: ProjectActivities {
                training,
                rollout,
                curation,
            },
            recording: Arc::new(RecordingSession::default()),
        })
    }
}

impl OpenProject {
    fn resolve_connection_update(
        update: SpiralConnectionUpdate,
    ) -> Result<SpiralConnectionSettings, OperationError> {
        let (endpoint, spfs, project) = update.into_parts();
        SpiralConnectionSettings::new(endpoint, spfs)
            .and_then(|settings| settings.with_project(&project))
            .map_err(OperationError::InvalidInput)
    }

    /// Present a connection to the browser without its secrets.
    fn present(connection: &SpiralConnectionSettings) -> SpiralSettings {
        SpiralSettings::new(
            connection.endpoint(),
            connection.spfs_endpoint(),
            connection.project_id().unwrap_or_default(),
        )
    }

    fn connection(&self) -> SpiralConnectionSettings {
        self.storage().project_store().connection_settings()
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
    fn recording(&self) -> &Arc<RecordingSession> {
        &self.recording
    }
    fn training(&self) -> &Arc<TrainingManager> {
        &self.activities.training
    }
    fn rollout(&self) -> &Arc<RolloutManager> {
        &self.activities.rollout
    }
    fn curation(&self) -> &Arc<CurationManager> {
        &self.activities.curation
    }

    pub(crate) async fn settings(&self) -> Result<DataSettings, OperationError> {
        let preferences = self
            .storage()
            .preferences()
            .load()
            .await
            .map_err(OperationError::External)?;
        Ok(DataSettings::new(
            Self::present(&self.connection()),
            TrainingRuntimeSettings::new(preferences.training().endpoint()),
            VideoSettings::new(preferences.video().enabled()),
        ))
    }

    /// Reach Spiral with this connection before Studio switches to it.
    async fn validate(&self) -> Result<(), OperationError> {
        let (available, detail) = self.storage().database().test().await;
        if available {
            Ok(())
        } else {
            Err(OperationError::External(detail))
        }
    }

    /// Runs keep the runtime they started on, so the endpoint applies to the
    /// next run and probe.
    async fn apply_preferences(
        &self,
        preferences: StoredStudioPreferences,
    ) -> Result<(), OperationError> {
        let endpoint_changed = self
            .storage()
            .preferences()
            .load()
            .await
            .map_err(OperationError::External)?
            .training()
            .endpoint()
            != preferences.training().endpoint();
        self.storage()
            .preferences()
            .save(&preferences)
            .await
            .map_err(OperationError::External)?;
        if endpoint_changed {
            self.training().refresh_readiness().await;
        }
        Ok(())
    }

    /// Whether recording, training, or rollout is active.
    pub(crate) async fn busy(&self) -> Result<bool, OperationError> {
        Ok(self.recording().active()
            || self.rollout().is_active()
            || self.training().active().await?)
    }

    pub(crate) async fn shutdown(&self) {
        self.curation().shutdown();
        self.rollout().shutdown().await;
    }
}

/// The connection Studio starts with, and every project opened through it.
struct OpenedProjects {
    saved: SpiralConnectionSettings,
    projects: Vec<Arc<OpenProject>>,
}

/// Opens the projects requests name, and saves the connection Studio starts
/// with. A project stays open, with its running work, until the Spiral account
/// changes or an import replaces its data.
pub(crate) struct ProjectRegistry {
    factory: ProjectFactory,
    backend_state: Arc<BackendStateStore>,
    opened: Mutex<OpenedProjects>,
}

impl ProjectRegistry {
    pub(crate) fn new(
        factory: ProjectFactory,
        backend_state: Arc<BackendStateStore>,
        saved: SpiralConnectionSettings,
    ) -> Arc<Self> {
        Arc::new(Self {
            factory,
            backend_state,
            opened: Mutex::new(OpenedProjects {
                saved,
                projects: Vec::new(),
            }),
        })
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

    /// The saved connection, which names the project a browser starts on.
    pub(crate) fn saved_connection(&self) -> SpiralConnectionSettings {
        self.opened.recover_lock().saved.clone()
    }

    pub(crate) fn saved_settings(&self) -> SpiralSettings {
        OpenProject::present(&self.saved_connection())
    }

    /// The project `key` names, reached with the saved credentials. A key for
    /// another endpoint comes from a browser that has not seen a later save.
    pub(crate) fn open(&self, key: &ProjectKey) -> Result<Arc<OpenProject>, OperationError> {
        let mut opened = self.opened.recover_lock();
        if key.endpoint() != opened.saved.endpoint() {
            return Err(OperationError::StaleProject(
                "Studio now uses another Spiral endpoint. Reload to continue.".to_owned(),
            ));
        }
        let connection = if key.is_chosen() {
            opened
                .saved
                .clone()
                .with_project(key.project_id())
                .map_err(OperationError::InvalidInput)?
        } else {
            opened.saved.clone().without_project()
        };
        if let Some(project) = opened
            .projects
            .iter()
            .find(|project| project.connection() == connection)
        {
            return Ok(project.clone());
        }
        let project = self.factory.create(connection)?;
        opened.projects.push(project.clone());
        Ok(project)
    }

    pub(crate) fn opened(&self) -> Vec<Arc<OpenProject>> {
        self.opened.recover_lock().projects.clone()
    }

    /// Preferences for the same connection apply in place. A new connection is
    /// validated and saved first; the preferences then belong to its project,
    /// which comes back beside the saved settings.
    pub(crate) async fn update_settings(
        &self,
        origin: &OpenProject,
        update: SettingsUpdate,
    ) -> Result<(DataSettings, Option<Arc<OpenProject>>), OperationError> {
        let (connection, training, video) = update.into_parts();
        let connection = OpenProject::resolve_connection_update(connection)?;
        let preferences =
            StoredStudioPreferences::from_settings(training.endpoint(), video.enabled())
                .map_err(OperationError::InvalidInput)?;
        if connection == origin.connection() {
            origin.apply_preferences(preferences).await?;
            return Ok((origin.settings().await?, None));
        }
        let candidate = self.factory.create(connection)?;
        candidate.validate().await?;
        let target = self.switch(origin, candidate).await?;
        target.apply_preferences(preferences).await?;
        Ok((target.settings().await?, Some(target)))
    }

    /// Import into a freshly opened project, so no cache of the replaced data survives,
    /// and put it in place of `origin`.
    pub(crate) async fn import(
        &self,
        origin: &Arc<OpenProject>,
        document: &[u8],
    ) -> Result<
        (
            spiral_robotics_common::datasets::ProjectImportSummary,
            Arc<OpenProject>,
        ),
        OperationError,
    > {
        let hardware = self.hardware().state();
        if origin.busy().await?
            || hardware.recording().is_some()
            || !matches!(hardware.mode(), HardwareModeSelection::Idle)
        {
            return Err(OperationError::conflict(
                "Stop recording, rollout, and training before importing a project.",
            ));
        }
        // The import replaces the robot configurations connected robots use.
        for (id, connection) in hardware.connections() {
            if connection.connected() {
                self.hardware()
                    .disconnect(id)
                    .await
                    .map_err(OperationError::External)?;
            }
        }
        let candidate = self.factory.create(origin.connection())?;
        let summary = candidate
            .storage()
            .transfer()
            .import_project(document)
            .await
            .map_err(OperationError::External)?;
        if let Some(slot) = self
            .opened
            .recover_lock()
            .projects
            .iter_mut()
            .find(|project| Arc::ptr_eq(project, origin))
        {
            *slot = candidate.clone();
        }
        let replaced = origin.clone();
        tokio::spawn(async move { replaced.shutdown().await });
        Ok((summary, candidate))
    }

    /// Save `candidate`'s connection as the one Studio starts with and open its
    /// project. Work in other projects keeps running unless the Spiral account
    /// itself changes, which retires every open project.
    async fn switch(
        &self,
        origin: &OpenProject,
        candidate: Arc<OpenProject>,
    ) -> Result<Arc<OpenProject>, OperationError> {
        let connection = candidate.connection();
        if connection != origin.connection() {
            self.release_robots(origin).await?;
        }
        let account = connection.clone().without_project();
        if self.saved_connection().without_project() != account {
            self.refuse_while_busy("changing the Spiral connection")
                .await?;
        }
        self.backend_state
            .save_connection(&connection)
            .await
            .map_err(OperationError::Internal)?;
        let (project, retired) = {
            let mut opened = self.opened.recover_lock();
            let retired = if opened.saved.clone().without_project() == account {
                Vec::new()
            } else {
                std::mem::take(&mut opened.projects)
            };
            opened.saved = connection.clone();
            let existing = opened
                .projects
                .iter()
                .find(|project| project.connection() == connection)
                .cloned();
            let project = existing.unwrap_or_else(|| {
                opened.projects.push(candidate.clone());
                candidate
            });
            (project, retired)
        };
        for retired_project in retired {
            tokio::spawn(async move { retired_project.shutdown().await });
        }
        Ok(project)
    }

    /// Run `change` to the signed-in Spiral account, then close every project
    /// opened as the previous user and forget the saved project, which was theirs.
    pub(crate) async fn change_account<T>(
        &self,
        origin: &OpenProject,
        change: impl Future<Output = Result<T, OperationError>>,
    ) -> Result<T, OperationError> {
        self.release_robots(origin).await?;
        self.refuse_while_busy("changing the Spiral account")
            .await?;
        let changed = change.await?;
        let saved = self.saved_connection().without_project();
        self.backend_state
            .save_connection(&saved)
            .await
            .map_err(OperationError::Internal)?;
        let retired = {
            let mut opened = self.opened.recover_lock();
            opened.saved = saved;
            std::mem::take(&mut opened.projects)
        };
        for retired_project in retired {
            tokio::spawn(async move { retired_project.shutdown().await });
        }
        Ok(changed)
    }

    async fn refuse_while_busy(&self, action: &str) -> Result<(), OperationError> {
        for project in self.opened() {
            if project.busy().await? {
                return Err(OperationError::conflict(format!(
                    "Stop recording, rollout, and training before {action}."
                )));
            }
        }
        Ok(())
    }

    /// Robots set up in the project being left stay with it: the operator
    /// disconnects them. Robots no project shows are disconnected here.
    async fn release_robots(&self, origin: &OpenProject) -> Result<(), OperationError> {
        let hardware = self.hardware().state();
        let visible = origin.workspace().try_list().await.ok().map(|robots| {
            robots
                .into_iter()
                .map(|robot| robot.id().to_owned())
                .collect::<BTreeSet<_>>()
        });
        match DestinationChangeContext::new(&hardware, visible.as_ref()).admission() {
            DestinationChangeAdmission::Ready => Ok(()),
            DestinationChangeAdmission::Busy => Err(OperationError::conflict(
                "Stop recording and rollout, and disconnect this project's robots before changing projects.",
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

    pub(crate) async fn shutdown(&self) {
        let projects = std::mem::take(&mut self.opened.recover_lock().projects);
        for project in projects {
            project.shutdown().await;
        }
    }
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

    pub(crate) const ENDPOINT: &str = "https://api.spiraldb.dev";

    pub(crate) struct ProjectFixture {
        pub(crate) projects: Arc<ProjectRegistry>,
        /// Holds the backend database's directory for the fixture's lifetime.
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
            let factory = ProjectFactory::new(
                hardware,
                ProjectDataAccess::new(
                    spiral_robotics_storage::StudioStore::in_memory(),
                    spiral_robotics_storage::SpiralAccessPublisher::new().1,
                    tokio::sync::watch::channel(ProviderKeys::default()).1,
                ),
            );
            let backend_state = Arc::new(BackendStateStore::new(
                spiral_robotics_storage::StudioStore::at(directory.path().join("studio.sqlite3")),
            ));
            let connection = SpiralConnectionSettings::new(
                ENDPOINT.to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap();
            Self {
                projects: ProjectRegistry::new(factory, backend_state, connection),
                _directory: directory,
            }
        }

        pub(crate) fn open(&self, project_id: &str) -> Arc<OpenProject> {
            self.projects
                .open(&ProjectKey::new(ENDPOINT, project_id))
                .unwrap()
        }

        /// What a request reaches: these projects, signed out, with keys
        /// saved in memory.
        pub(crate) fn context(&self) -> crate::projects::context::StudioContext {
            let keys = spiral_robotics_storage::ProviderKeyring::new(
                Arc::new(BackendStateStore::new(
                    spiral_robotics_storage::StudioStore::in_memory(),
                )),
                ProviderKeys::default(),
            );
            crate::projects::context::StudioContext::new(
                self.projects.clone(),
                crate::account::SpiralAccount::offline(),
                Arc::new(keys),
            )
        }
    }

    #[tokio::test]
    async fn requests_reach_the_project_they_name_and_share_it() {
        let fixture = ProjectFixture::new();
        let first = fixture.open("project_a");
        assert!(Arc::ptr_eq(&first, &fixture.open("project_a")));
        let second = fixture.open("project_b");
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.connection().project_id(), Some("project_a"));
        assert_eq!(second.connection().project_id(), Some("project_b"));
        assert_eq!(fixture.open("").connection().project_id(), None);
        assert_eq!(fixture.projects.opened().len(), 3);
        assert!(matches!(
            fixture
                .projects
                .open(&ProjectKey::new("https://other.spiraldb.dev", "project_a")),
            Err(OperationError::StaleProject(_))
        ));
        assert!(matches!(
            fixture
                .projects
                .open(&ProjectKey::new(ENDPOINT, "not/a/project")),
            Err(OperationError::InvalidInput(_))
        ));
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn training_endpoint_changes_apply_in_place() {
        let fixture = ProjectFixture::new();
        let project = fixture.open("");
        let preferences =
            StoredStudioPreferences::from_settings("http://127.0.0.1:5050", false).unwrap();
        project.apply_preferences(preferences).await.unwrap();
        assert!(Arc::ptr_eq(&project, &fixture.open("")));
        let settings = project.settings().await.unwrap();
        assert_eq!(settings.training().endpoint(), "http://127.0.0.1:5050");
        assert!(!settings.video().enabled());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn settings_present_the_project_connection_and_preferences() {
        let fixture = ProjectFixture::new();
        let connection = SpiralConnectionSettings::new(
            ENDPOINT.to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
        )
        .unwrap()
        .with_project("configured_project")
        .unwrap();
        let project = fixture.projects.factory.create(connection).unwrap();
        let preferences =
            StoredStudioPreferences::from_settings("http://127.0.0.1:5050", false).unwrap();
        project
            .storage()
            .preferences()
            .save(&preferences)
            .await
            .unwrap();
        let settings = project.settings().await.unwrap();
        assert_eq!(settings.spiral().project_id(), "configured_project");
        assert_eq!(settings.spiral().endpoint(), "https://api.spiraldb.dev");
        assert_eq!(
            settings.spiral().spfs_endpoint(),
            "https://spfs.spiraldb.dev"
        );
        assert_eq!(settings.training().endpoint(), "http://127.0.0.1:5050");
        assert!(!settings.video().enabled());
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
