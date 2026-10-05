//! Every project this process has opened, each for the browser session that
//! opened it and keyed by the connection that reaches it. Requests name their
//! project; nothing here is "current".

pub(crate) mod bootstrap;
pub(crate) mod context;
pub(crate) mod http;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::settings::{
    DataSettings, SettingsUpdate, SpiralSettings, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_hardware::{
    HardwareHandle, HardwareState, ModeSelection as HardwareModeSelection,
};
use spiral_robotics_storage::episodes::RecordingBuffer;
use spiral_robotics_storage::{
    ProjectStore, ProviderKeys, SpiralAccess, SpiralConnection, SpiralConnectionSettings,
    StorageManager, StoredStudioPreferences,
};

use crate::OperationError;
use crate::account::SpiralSession;
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

/// How every project reaches its data: Studio's local store and the provider
/// keys the driver works with. Each project reaches Spiral as its session's user.
pub(crate) struct ProjectDataAccess {
    studio: spiral_robotics_storage::StudioStore,
    keys: tokio::sync::watch::Receiver<ProviderKeys>,
}

impl ProjectDataAccess {
    pub(crate) fn new(
        studio: spiral_robotics_storage::StudioStore,
        keys: tokio::sync::watch::Receiver<ProviderKeys>,
    ) -> Self {
        Self { studio, keys }
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

    /// A project that reaches Spiral with `access`.
    fn create(
        &self,
        settings: SpiralConnectionSettings,
        access: SpiralAccess,
    ) -> Result<Arc<OpenProject>, OperationError> {
        let connection = SpiralConnection::new(settings, access);
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

/// A project one browser session opened.
struct SessionProject {
    session: Arc<SpiralSession>,
    project: Arc<OpenProject>,
}

/// A project a request reached: one its session had open, or one no session
/// had open before.
pub(crate) enum ProjectOpening {
    Opened(Arc<OpenProject>),
    FirstInStudio(Arc<OpenProject>),
}

impl ProjectOpening {
    fn into_project(self) -> Arc<OpenProject> {
        match self {
            Self::Opened(project) | Self::FirstInStudio(project) => project,
        }
    }
}

/// Opens the projects requests name, each as the session that names it. A
/// project stays open, with its running work, until its session ends or
/// changes organization, or an import replaces its data.
pub(crate) struct ProjectRegistry {
    factory: ProjectFactory,
    /// The Spiral endpoints every project reaches, fixed while Studio runs.
    endpoints: SpiralConnectionSettings,
    opened: Mutex<Vec<SessionProject>>,
}

impl ProjectRegistry {
    pub(crate) fn new(factory: ProjectFactory, endpoints: SpiralConnectionSettings) -> Arc<Self> {
        Arc::new(Self {
            factory,
            endpoints: endpoints.without_project(),
            opened: Mutex::new(Vec::new()),
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

    /// The endpoints, and the project `session`'s browser works on.
    pub(crate) async fn session_settings(
        &self,
        session: &SpiralSession,
    ) -> Result<SpiralSettings, OperationError> {
        Ok(SpiralSettings::new(
            self.endpoints.endpoint(),
            self.endpoints.spfs_endpoint(),
            session.project_id().await?,
        ))
    }

    /// The project `key` names, as `session`'s user, who must be allowed to
    /// open it. A key for another endpoint comes from a browser that has not
    /// seen Studio restart on new endpoints.
    pub(crate) async fn open(
        &self,
        session: &Arc<SpiralSession>,
        key: &ProjectKey,
    ) -> Result<ProjectOpening, OperationError> {
        if key.endpoint() != self.endpoints.endpoint() {
            return Err(OperationError::StaleProject(
                "Studio now uses another Spiral endpoint. Reload to continue.".to_owned(),
            ));
        }
        let connection = if key.is_chosen() {
            self.endpoints
                .clone()
                .with_project(key.project_id())
                .map_err(OperationError::InvalidInput)?
        } else {
            self.endpoints.clone()
        };
        if let Some(project) = self.find(session, &connection) {
            return Ok(ProjectOpening::Opened(project));
        }
        if let Some(project_id) = connection.project_id() {
            session.admit(project_id).await?;
        }
        let candidate = self.factory.create(connection, session.access())?;
        Ok(self.adopt(session, candidate))
    }

    /// Every open project, with the session that opened it.
    pub(crate) fn opened(&self) -> Vec<(Arc<SpiralSession>, Arc<OpenProject>)> {
        self.opened
            .recover_lock()
            .iter()
            .map(|opened| (opened.session.clone(), opened.project.clone()))
            .collect()
    }

    /// Preferences for the same project apply in place. Another project is
    /// validated first; the session then works on it, and the preferences
    /// belong to it. It comes back beside the saved settings.
    pub(crate) async fn update_settings(
        &self,
        session: &Arc<SpiralSession>,
        origin: &OpenProject,
        update: SettingsUpdate,
    ) -> Result<(DataSettings, Option<Arc<OpenProject>>), OperationError> {
        let (project_id, training, video) = update.into_parts();
        let connection = self
            .endpoints
            .clone()
            .with_project(&project_id)
            .map_err(OperationError::InvalidInput)?;
        let preferences =
            StoredStudioPreferences::from_settings(training.endpoint(), video.enabled())
                .map_err(OperationError::InvalidInput)?;
        let project_id = connection.project_id().unwrap_or_default().to_owned();
        if connection == origin.connection() {
            origin.apply_preferences(preferences).await?;
            session.choose_project(&project_id).await?;
            return Ok((origin.settings().await?, None));
        }
        session.admit(&project_id).await?;
        let candidate = self.factory.create(connection, session.access())?;
        candidate.validate().await?;
        self.release_robots(origin).await?;
        session.choose_project(&project_id).await?;
        let target = self.adopt(session, candidate).into_project();
        target.apply_preferences(preferences).await?;
        Ok((target.settings().await?, Some(target)))
    }

    /// Import into a freshly opened project, so no cache of the replaced data survives,
    /// and put it in place of `origin`.
    pub(crate) async fn import(
        &self,
        session: &Arc<SpiralSession>,
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
        let candidate = self.factory.create(origin.connection(), session.access())?;
        let summary = candidate
            .storage()
            .transfer()
            .import_project(document)
            .await
            .map_err(OperationError::External)?;
        if let Some(slot) = self
            .opened
            .recover_lock()
            .iter_mut()
            .find(|opened| Arc::ptr_eq(&opened.project, origin))
        {
            slot.project = candidate.clone();
        }
        let replaced = origin.clone();
        tokio::spawn(async move { replaced.shutdown().await });
        Ok((summary, candidate))
    }

    /// Run `change` to `session`'s Spiral account, such as signing out or
    /// another organization, then close every project the session opened as
    /// it was before.
    pub(crate) async fn change_account<T>(
        &self,
        session: &Arc<SpiralSession>,
        origin: &OpenProject,
        change: impl Future<Output = Result<T, OperationError>>,
    ) -> Result<T, OperationError> {
        self.release_robots(origin).await?;
        self.refuse_while_busy(session, "changing the Spiral account")
            .await?;
        let changed = change.await?;
        Self::shut_down(self.retire(|opened| Arc::ptr_eq(&opened.session, session)));
        Ok(changed)
    }

    /// `session`'s project for `connection`, if it has it open.
    fn find(
        &self,
        session: &Arc<SpiralSession>,
        connection: &SpiralConnectionSettings,
    ) -> Option<Arc<OpenProject>> {
        self.opened
            .recover_lock()
            .iter()
            .find(|opened| {
                Arc::ptr_eq(&opened.session, session) && opened.project.connection() == *connection
            })
            .map(|opened| opened.project.clone())
    }

    /// Keep `candidate` open for `session`, unless a concurrent request opened
    /// the same project first. Projects of sessions that ended close here.
    fn adopt(&self, session: &Arc<SpiralSession>, candidate: Arc<OpenProject>) -> ProjectOpening {
        Self::shut_down(self.retire(|opened| opened.session.ended()));
        let connection = candidate.connection();
        let mut opened = self.opened.recover_lock();
        if let Some(existing) = opened.iter().find(|opened| {
            Arc::ptr_eq(&opened.session, session) && opened.project.connection() == connection
        }) {
            return ProjectOpening::Opened(existing.project.clone());
        }
        let first = opened
            .iter()
            .all(|opened| opened.project.connection() != connection);
        opened.push(SessionProject {
            session: session.clone(),
            project: candidate.clone(),
        });
        if first {
            ProjectOpening::FirstInStudio(candidate)
        } else {
            ProjectOpening::Opened(candidate)
        }
    }

    /// Stop tracking the projects `retired` selects, and return them.
    fn retire(&self, retired: impl Fn(&SessionProject) -> bool) -> Vec<Arc<OpenProject>> {
        self.opened
            .recover_lock()
            .extract_if(.., |opened| retired(opened))
            .map(|opened| opened.project)
            .collect()
    }

    fn shut_down(projects: Vec<Arc<OpenProject>>) {
        for project in projects {
            tokio::spawn(async move { project.shutdown().await });
        }
    }

    async fn refuse_while_busy(
        &self,
        session: &Arc<SpiralSession>,
        action: &str,
    ) -> Result<(), OperationError> {
        let projects: Vec<_> = self
            .opened()
            .into_iter()
            .filter(|(owner, _)| Arc::ptr_eq(owner, session))
            .collect();
        for (_, project) in projects {
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
        for project in self.retire(|_| true) {
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
    use crate::account::fake::FakeSpiral;

    pub(crate) const ENDPOINT: &str = "https://api.spiraldb.dev";

    /// Projects opened by a browser signed in to a fake Spiral.
    pub(crate) struct ProjectFixture {
        pub(crate) projects: Arc<ProjectRegistry>,
        sessions: Arc<crate::account::SpiralSessions>,
        session_id: String,
        spiral: FakeSpiral,
    }

    impl ProjectFixture {
        pub(crate) async fn new() -> Self {
            let spiral = FakeSpiral::start().await;
            let sessions = Arc::new(spiral.sessions(spiral_robotics_storage::SessionStore::new(
                spiral_robotics_storage::StudioStore::in_memory(),
            )));
            let session_id = FakeSpiral::sign_in(&sessions).await;
            let hardware = ProjectHardware::new(
                Arc::new(HardwareHandle::spawn(Default::default())),
                Arc::new(CameraManager::new()),
                VideoEncoderSettings::new("ffmpeg".to_owned(), "libx264".to_owned()),
            );
            let factory = ProjectFactory::new(
                hardware,
                ProjectDataAccess::new(
                    spiral_robotics_storage::StudioStore::in_memory(),
                    tokio::sync::watch::channel(ProviderKeys::default()).1,
                ),
            );
            let endpoints = SpiralConnectionSettings::new(
                ENDPOINT.to_owned(),
                "https://spfs.spiraldb.dev".to_owned(),
            )
            .unwrap();
            Self {
                projects: ProjectRegistry::new(factory, endpoints),
                sessions,
                session_id,
                spiral,
            }
        }

        /// The Spiral its sessions sign in to.
        pub(crate) fn spiral(&self) -> &FakeSpiral {
            &self.spiral
        }

        /// The fixture's signed-in browser session.
        pub(crate) async fn session(&self) -> Arc<SpiralSession> {
            self.sessions.find(&self.session_id).await.unwrap()
        }

        /// Another browser, signed in as the same user.
        pub(crate) async fn another_session(&self) -> Arc<SpiralSession> {
            let id = FakeSpiral::sign_in(&self.sessions).await;
            self.sessions.find(&id).await.unwrap()
        }

        /// The cookie the fixture's browser sends with every request.
        pub(crate) fn cookie(&self) -> String {
            format!(
                "{}={}",
                crate::account::sign_in::BrowserCookies::SESSION,
                self.session_id
            )
        }

        pub(crate) async fn open(&self, project_id: &str) -> Arc<OpenProject> {
            self.projects
                .open(
                    &self.session().await,
                    &ProjectKey::new(ENDPOINT, project_id),
                )
                .await
                .unwrap()
                .into_project()
        }

        /// What a request reaches: these projects and sessions, with keys
        /// saved in memory.
        pub(crate) fn context(&self) -> crate::projects::context::StudioContext {
            let keys = spiral_robotics_storage::ProviderKeyring::new(
                Arc::new(spiral_robotics_storage::BackendStateStore::new(
                    spiral_robotics_storage::StudioStore::in_memory(),
                )),
                ProviderKeys::default(),
            );
            crate::projects::context::StudioContext::new(
                self.projects.clone(),
                self.sessions.clone(),
                Arc::new(keys),
            )
        }
    }

    #[tokio::test]
    async fn each_session_opens_the_projects_it_names_and_may_see() {
        let fixture = ProjectFixture::new().await;
        let session = fixture.session().await;
        let first = match fixture
            .projects
            .open(&session, &ProjectKey::new(ENDPOINT, "project_a"))
            .await
            .unwrap()
        {
            ProjectOpening::FirstInStudio(project) => project,
            ProjectOpening::Opened(_) => panic!("project_a was not open yet"),
        };
        assert!(Arc::ptr_eq(&first, &fixture.open("project_a").await));
        let second = fixture.open("project_b").await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.connection().project_id(), Some("project_a"));
        assert_eq!(second.connection().project_id(), Some("project_b"));
        assert_eq!(fixture.open("").await.connection().project_id(), None);
        assert_eq!(fixture.projects.opened().len(), 3);
        // Another browser reaches Spiral with its own token, so it opens its own.
        let elsewhere = fixture.another_session().await;
        let theirs = fixture
            .projects
            .open(&elsewhere, &ProjectKey::new(ENDPOINT, "project_a"))
            .await
            .unwrap();
        assert!(
            matches!(&theirs, ProjectOpening::Opened(project) if !Arc::ptr_eq(project, &first))
        );
        for (key, refused) in [
            (
                ProjectKey::new("https://other.spiraldb.dev", "project_a"),
                "stale",
            ),
            (ProjectKey::new(ENDPOINT, "not/a/project"), "invalid"),
            (ProjectKey::new(ENDPOINT, "forbidden_arms"), "hidden"),
        ] {
            let result = fixture.projects.open(&session, &key).await;
            assert!(
                matches!(
                    (refused, &result),
                    ("stale", Err(OperationError::StaleProject(_)))
                        | ("invalid", Err(OperationError::InvalidInput(_)))
                        | ("hidden", Err(OperationError::NotFound(_)))
                ),
                "{refused}"
            );
        }
        assert_eq!(fixture.projects.opened().len(), 4);
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn changing_the_account_closes_only_that_sessions_projects() {
        let fixture = ProjectFixture::new().await;
        let session = fixture.session().await;
        let mine = fixture.open("project_a").await;
        let elsewhere = fixture.another_session().await;
        fixture
            .projects
            .open(&elsewhere, &ProjectKey::new(ENDPOINT, "project_a"))
            .await
            .unwrap();
        let identity = fixture
            .projects
            .change_account(&session, &mine, session.switch_organization("org_2"))
            .await
            .unwrap();
        assert_eq!(identity.organization().id(), "org_2");
        let remaining = fixture.projects.opened();
        assert_eq!(remaining.len(), 1);
        assert!(Arc::ptr_eq(&remaining[0].0, &elsewhere));
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn training_endpoint_changes_apply_in_place() {
        let fixture = ProjectFixture::new().await;
        let project = fixture.open("").await;
        let preferences =
            StoredStudioPreferences::from_settings("http://127.0.0.1:5050", false).unwrap();
        project.apply_preferences(preferences).await.unwrap();
        assert!(Arc::ptr_eq(&project, &fixture.open("").await));
        let settings = project.settings().await.unwrap();
        assert_eq!(settings.training().endpoint(), "http://127.0.0.1:5050");
        assert!(!settings.video().enabled());
        fixture.projects.hardware().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn settings_present_the_project_connection_and_preferences() {
        let fixture = ProjectFixture::new().await;
        let connection = SpiralConnectionSettings::new(
            ENDPOINT.to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
        )
        .unwrap()
        .with_project("configured_project")
        .unwrap();
        let project = fixture
            .projects
            .factory
            .create(connection, fixture.session().await.access())
            .unwrap();
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
