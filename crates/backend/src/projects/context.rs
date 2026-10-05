//! What a request can reach: Studio-wide services, and the project it names.
use crate::account::{SpiralSession, SpiralSessions};
use crate::curation::manager::CurationManager;
use crate::projects::{OpenProject, ProjectOpening, ProjectRegistry};
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::operations::RobotOperations;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::RolloutManager;
use crate::training::manager::TrainingManager;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_hardware::HardwareHandle;
use spiral_robotics_storage::{ProviderKeyring, StorageManager, episodes::RecordingBuffer};
use std::sync::Arc;

/// Services shared by every request: the open projects, the hardware they
/// share, the browsers' Spiral sessions, and the provider keys.
#[derive(Clone)]
pub(crate) struct StudioContext {
    projects: Arc<ProjectRegistry>,
    sessions: Arc<SpiralSessions>,
    keys: Arc<ProviderKeyring>,
}

impl StudioContext {
    pub(crate) fn new(
        projects: Arc<ProjectRegistry>,
        sessions: Arc<SpiralSessions>,
        keys: Arc<ProviderKeyring>,
    ) -> Self {
        Self {
            projects,
            sessions,
            keys,
        }
    }

    /// The project `key` names, as `session`'s user. The first session to open
    /// a project reconnects the robots saved in it.
    pub(crate) async fn open(
        &self,
        session: &Arc<SpiralSession>,
        key: &ProjectKey,
    ) -> Result<ProjectContext, crate::OperationError> {
        let opened = match self.projects.open(session, key).await? {
            ProjectOpening::Opened(project) => self.with_project(session.clone(), project),
            ProjectOpening::FirstInStudio(project) => {
                let opened = self.with_project(session.clone(), project);
                RobotOperations::spawn_auto_connect(opened.clone());
                opened
            }
        };
        Ok(opened)
    }

    pub(crate) fn opened(&self) -> Vec<ProjectContext> {
        self.projects
            .opened()
            .into_iter()
            .map(|(session, project)| self.with_project(session, project))
            .collect()
    }

    fn with_project(
        &self,
        session: Arc<SpiralSession>,
        project: Arc<OpenProject>,
    ) -> ProjectContext {
        ProjectContext {
            project,
            session,
            studio: self.clone(),
        }
    }

    pub(crate) fn projects(&self) -> &Arc<ProjectRegistry> {
        &self.projects
    }
    pub(crate) fn sessions(&self) -> &Arc<SpiralSessions> {
        &self.sessions
    }
    pub(crate) fn keys(&self) -> &ProviderKeyring {
        &self.keys
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        self.projects.cameras()
    }
}

/// A request or task retains the project it named, and the session that
/// opened it, for its entire lifetime.
#[derive(Clone)]
pub(crate) struct ProjectContext {
    project: Arc<OpenProject>,
    session: Arc<SpiralSession>,
    studio: StudioContext,
}

impl ProjectContext {
    /// The same session and Studio services with another project, such as the
    /// one a settings change just opened.
    pub(crate) fn with_project(&self, project: Arc<OpenProject>) -> Self {
        self.studio.with_project(self.session.clone(), project)
    }

    pub(crate) fn project(&self) -> &Arc<OpenProject> {
        &self.project
    }
    pub(crate) fn session(&self) -> &Arc<SpiralSession> {
        &self.session
    }
    pub(crate) fn studio(&self) -> &StudioContext {
        &self.studio
    }
    pub(crate) fn projects(&self) -> &Arc<ProjectRegistry> {
        self.studio.projects()
    }
    pub(crate) fn workspace(&self) -> &Arc<RobotWorkspace> {
        self.project.workspace()
    }
    pub(crate) fn recording_buffer(&self) -> &Arc<RecordingBuffer> {
        self.project.recording_buffer()
    }
    pub(crate) fn storage(&self) -> &Arc<StorageManager> {
        self.project.storage()
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        self.projects().hardware()
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        self.studio.cameras()
    }
    pub(crate) fn recording_session(&self) -> &Arc<RecordingSession> {
        self.project.recording()
    }
    pub(crate) fn video_encoder(&self) -> &VideoEncoderSettings {
        self.projects().video_encoder()
    }
    pub(crate) fn training(&self) -> &Arc<TrainingManager> {
        self.project.training()
    }
    pub(crate) fn curation(&self) -> &Arc<CurationManager> {
        self.project.curation()
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        self.project.rollout()
    }
}
