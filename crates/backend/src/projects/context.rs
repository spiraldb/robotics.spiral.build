//! What a request can reach: Studio-wide services, and the project it names.
use crate::curation::manager::CurationManager;
use crate::exploration::manager::ExplorationManager;
use crate::projects::tasks::BackendTasks;
use crate::projects::{OpenProject, ProjectRegistry};
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::RolloutManager;
use crate::training::manager::TrainingManager;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_hardware::HardwareHandle;
use spiral_robotics_storage::{StorageManager, episodes::RecordingBuffer};
use std::sync::Arc;

/// Services shared by every request: the open projects, the hardware they
/// share, and the runtime that owns backend tasks.
#[derive(Clone)]
pub(crate) struct StudioContext {
    projects: Arc<ProjectRegistry>,
    tasks: BackendTasks,
}

impl StudioContext {
    pub(crate) fn new(projects: Arc<ProjectRegistry>) -> Self {
        Self {
            projects,
            tasks: BackendTasks::current(),
        }
    }

    pub(crate) fn open(&self, key: &ProjectKey) -> Result<ProjectContext, crate::OperationError> {
        Ok(self.with_project(self.projects.open(key)?))
    }

    pub(crate) fn opened(&self) -> Vec<ProjectContext> {
        self.projects
            .opened()
            .into_iter()
            .map(|project| self.with_project(project))
            .collect()
    }

    fn with_project(&self, project: Arc<OpenProject>) -> ProjectContext {
        ProjectContext {
            project,
            studio: self.clone(),
        }
    }

    pub(crate) fn projects(&self) -> &Arc<ProjectRegistry> {
        &self.projects
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        self.projects.cameras()
    }
}

/// A request or task retains the project it named for its entire lifetime.
#[derive(Clone)]
pub(crate) struct ProjectContext {
    project: Arc<OpenProject>,
    studio: StudioContext,
}

impl ProjectContext {
    /// The same Studio services with another project, such as the one a
    /// settings change just opened.
    pub(crate) fn with_project(&self, project: Arc<OpenProject>) -> Self {
        self.studio.with_project(project)
    }

    pub(crate) fn tasks(&self) -> &BackendTasks {
        &self.studio.tasks
    }

    /// Run a command to completion even if its HTTP caller disconnects.
    pub(crate) async fn execute<Output, Work>(
        &self,
        command: impl FnOnce(Self) -> Work + Send + 'static,
    ) -> Result<Output, crate::BackendCommandError>
    where
        Output: Send + 'static,
        Work: std::future::Future<Output = Result<Output, crate::BackendCommandError>> + Send,
    {
        let state = self.clone();
        self.tasks()
            .execute(async move { command(state).await })
            .await?
    }

    pub(crate) fn project(&self) -> &Arc<OpenProject> {
        &self.project
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
    pub(crate) fn workflow(&self) -> &Arc<WorkflowManager> {
        self.project.workflow()
    }
    pub(crate) fn curation(&self) -> &Arc<CurationManager> {
        self.project.curation()
    }
    /// Explorations are stored opaquely and keep no runtime state, so each
    /// caller gets a manager over this project's storage rather than a shared one.
    pub(crate) fn explorations(&self) -> ExplorationManager {
        ExplorationManager::new(self.storage().clone())
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        self.project.rollout()
    }
    pub(crate) fn preferences(&self) -> &Arc<crate::data::preferences::RuntimePreferences> {
        self.project.preferences()
    }
}
