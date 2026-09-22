//! Every project this process has opened, each keyed by the connection that
//! reaches it. Requests name their project; nothing here is "current".

pub(crate) mod bootstrap;
pub(crate) mod context;
pub(crate) mod http;
pub(crate) mod tasks;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::settings::{
    DataSettings, SpiralConnectionUpdate, SpiralSettings, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection as HardwareModeSelection,
};
use spiral_robotics_storage::episodes::RecordingBuffer;
use spiral_robotics_storage::{
    BackendStateStore, ProjectStore, SpiralConnectionSettings, SpiralCredentials, StorageManager,
    StoredStudioPreferences,
};

use crate::OperationError;
use crate::curation::manager::CurationManager;
use crate::data::settings::{DataSettingsUpdate, UpdateDataSettingsCommand};
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::{RolloutHardwareResources, RolloutManager};
use crate::sync::MutexRecovery;
use crate::training::manager::TrainingManager;
use crate::workflow::manager::WorkflowManager;

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

pub(crate) struct ProjectFactory {
    hardware: ProjectHardware,
    table_prefix: String,
    studio: spiral_robotics_storage::StudioStore,
}

impl ProjectFactory {
    pub(crate) fn new(
        hardware: ProjectHardware,
        table_prefix: String,
        studio: spiral_robotics_storage::StudioStore,
    ) -> Self {
        Self {
            hardware,
            table_prefix,
            studio,
        }
    }

    fn create(
        &self,
        connection: SpiralConnectionSettings,
    ) -> Result<Arc<OpenProject>, OperationError> {
        let database = Arc::new(spiral_robotics_storage::AdbcDatabase::new(
            connection.clone(),
        ));
        let store = Arc::new(
            ProjectStore::new(connection, self.table_prefix.clone(), database)
                .map_err(OperationError::External)?,
        );
        Ok(self.create_from_store(store))
    }

    fn create_from_store(&self, store: Arc<ProjectStore>) -> Arc<OpenProject> {
        let studio = self.studio.project(&store.connection_settings());
        let workspace = Arc::new(RobotWorkspace::new(studio.clone()));
        let recording_buffer = Arc::new(RecordingBuffer::new());
        let storage = Arc::new(StorageManager::new(store, studio));
        let training = Arc::new(TrainingManager::new(storage.clone()));
        let workflow = Arc::new(WorkflowManager::new(storage.clone()));
        let curation = Arc::new(CurationManager::new(storage.clone()));
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
                workflow,
                curation,
            },
            recording: Arc::new(RecordingSession::default()),
        })
    }
}

impl OpenProject {
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

    /// Present a connection to the browser without its secrets.
    fn present(connection: &SpiralConnectionSettings, table_prefix: &str) -> SpiralSettings {
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
        .with_table_prefix(table_prefix)
    }

    fn connection(&self) -> SpiralConnectionSettings {
        self.storage().connection_settings()
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
    fn workflow(&self) -> &Arc<WorkflowManager> {
        &self.activities.workflow
    }
    fn curation(&self) -> &Arc<CurationManager> {
        &self.activities.curation
    }

    pub(crate) async fn settings(&self) -> Result<DataSettings, OperationError> {
        let preferences = self
            .storage()
            .load_studio_preferences()
            .await
            .map_err(OperationError::External)?;
        Ok(DataSettings::new(
            Self::present(&self.connection(), self.storage().table_prefix()),
            TrainingRuntimeSettings::new(preferences.training().endpoint()),
            VideoSettings::new(preferences.video().enabled()),
        ))
    }

    /// Reach Spiral with this connection before Studio switches to it.
    async fn validate(&self) -> Result<(), OperationError> {
        let (available, detail) = self.storage().test_spiral().await;
        if available {
            Ok(())
        } else {
            Err(OperationError::External(detail))
        }
    }

    /// Runs are followed through the current endpoint, so it changes only
    /// between runs. History loads first: after a restart the runs it left
    /// active are not known until then.
    async fn apply_preferences(
        &self,
        preferences: StoredStudioPreferences,
    ) -> Result<(), OperationError> {
        let endpoint_changed = self
            .storage()
            .load_studio_preferences()
            .await
            .map_err(OperationError::External)?
            .training()
            .endpoint()
            != preferences.training().endpoint();
        if endpoint_changed {
            self.training()
                .ensure_loaded()
                .await
                .map_err(OperationError::External)?;
            if self.training_active() {
                return Err(OperationError::conflict(
                    "Wait for training runs to finish before changing the training endpoint.",
                ));
            }
        }
        self.storage()
            .save_studio_preferences(&preferences)
            .await
            .map_err(OperationError::External)?;
        if endpoint_changed {
            self.training().refresh_readiness().await;
        }
        Ok(())
    }

    fn training_active(&self) -> bool {
        self.training()
            .snapshot()
            .runs()
            .iter()
            .any(|run| run.phase().active())
    }

    /// Whether recording, training, or rollout is active.
    pub(crate) fn busy(&self) -> bool {
        self.recording().active() || self.training_active() || self.rollout().is_active()
    }

    pub(crate) async fn shutdown(&self) {
        self.workflow().shutdown().await;
        self.curation().shutdown().await;
        self.training().shutdown().await;
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
        OpenProject::present(&self.saved_connection(), &self.factory.table_prefix)
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

    pub(crate) async fn update_connection(
        &self,
        origin: &OpenProject,
        update: SpiralConnectionUpdate,
    ) -> Result<Arc<OpenProject>, OperationError> {
        let connection = OpenProject::resolve_connection_update(update, &origin.connection())?;
        let candidate = self.factory.create(connection)?;
        candidate.validate().await?;
        self.switch(origin, candidate).await
    }

    /// Preferences for the same connection apply in place. A new connection is
    /// validated and saved first; the preferences then belong to its project.
    pub(crate) async fn update_settings(
        &self,
        origin: &OpenProject,
        command: UpdateDataSettingsCommand,
    ) -> Result<DataSettingsUpdate, OperationError> {
        let command = command.validated()?;
        let connection =
            OpenProject::resolve_connection_update((&command).into(), &origin.connection())?;
        let preferences = StoredStudioPreferences::from_settings(
            command.training_endpoint(),
            command.video_enabled(),
        )
        .map_err(OperationError::InvalidInput)?;
        if connection == origin.connection() {
            origin.apply_preferences(preferences).await?;
            return Ok(DataSettingsUpdate::new(origin.settings().await?, None));
        }
        let candidate = self.factory.create(connection)?;
        candidate.validate().await?;
        let target = self.switch(origin, candidate).await?;
        target.apply_preferences(preferences).await?;
        Ok(DataSettingsUpdate::new(
            target.settings().await?,
            Some(target),
        ))
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
        if origin.busy()
            || hardware.recording().is_some()
            || !matches!(hardware.mode(), HardwareModeSelection::Idle)
        {
            return Err(OperationError::conflict(
                "Stop recording, replay, rollout, and training before importing a project.",
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
            .import_project_bundle(document)
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
        {
            let opened = self.opened.recover_lock();
            if opened.saved.clone().without_project() != account
                && opened.projects.iter().any(|project| project.busy())
            {
                return Err(OperationError::conflict(
                    "Stop recording, rollout, and training before changing the Spiral connection.",
                ));
            }
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
                "Stop recording, replay, and rollout, and disconnect this project's robots before changing projects.",
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
                "project_lifetime_test".to_owned(),
                spiral_robotics_storage::StudioStore::in_memory(),
            );
            let backend_state = Arc::new(BackendStateStore::new(
                spiral_robotics_storage::StudioStore::at(directory.path().join("studio.sqlite3")),
                directory.path().join("backend.json"),
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
            let resolved = OpenProject::resolve_connection_update(decoded, &previous).unwrap();
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
        let repaired = OpenProject::resolve_connection_update(update.clone(), &previous).unwrap();
        assert_eq!(repaired.project_id(), Some("new"));
        assert!(repaired.credentials() == previous.credentials());
        assert!(
            OpenProject::resolve_connection_update(
                update.clone().with_client_credentials("different", ""),
                &previous,
            )
            .is_err()
        );
        assert!(
            OpenProject::resolve_connection_update(
                update.with_clear_client_secret(true),
                &previous,
            )
            .unwrap()
            .credentials()
            .is_none()
        );
    }

    #[tokio::test]
    async fn settings_present_the_project_connection_and_preferences_without_secrets() {
        let fixture = ProjectFixture::new();
        let connection = SpiralConnectionSettings::new(
            ENDPOINT.to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
        )
        .unwrap()
        .with_project("configured_project")
        .unwrap()
        .with_credentials(Some(SpiralCredentials::new(
            "client".to_owned(),
            "private-secret".to_owned(),
        )));
        let project = fixture.projects.factory.create(connection).unwrap();
        let preferences =
            StoredStudioPreferences::from_settings("http://127.0.0.1:5050", false).unwrap();
        project
            .storage()
            .save_studio_preferences(&preferences)
            .await
            .unwrap();
        let settings = project.settings().await.unwrap();
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
