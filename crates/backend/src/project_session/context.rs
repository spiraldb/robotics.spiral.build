//! Resources and admission for one immutable project identity, independent of delivery.
use super::ProjectReadiness;
use crate::curation::manager::CurationManager;
use crate::data::catalog_lease::CatalogMutationLease;
use crate::exploration::manager::ExplorationManager;
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::RolloutManager;
use crate::training::manager::TrainingManager;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_hardware::HardwareHandle;
use spiral_robotics_storage::{StorageManager, episodes::RecordingBuffer};
use std::sync::Arc;
/// A request or task retains one project for its entire lifetime.
#[derive(Clone)]
pub(crate) struct ProjectContext {
    project: Arc<crate::project_session::ProjectSession>,
    projects: Arc<crate::project_session::ProjectCoordinator>,
    tasks: crate::project_session::tasks::BackendTasks,
}

impl ProjectContext {
    pub(crate) fn new(projects: Arc<crate::project_session::ProjectCoordinator>) -> Self {
        Self {
            project: projects.active(),
            projects,
            tasks: crate::project_session::tasks::BackendTasks::current(),
        }
    }

    pub(crate) fn current(&self) -> Self {
        Self {
            project: self.projects.active(),
            projects: self.projects.clone(),
            tasks: self.tasks.clone(),
        }
    }

    pub(crate) fn for_session(&self, session_id: &str) -> Result<Self, crate::OperationError> {
        let project = self.projects.resolve(session_id)?;
        Ok(Self {
            project,
            projects: self.projects.clone(),
            tasks: self.tasks.clone(),
        })
    }

    pub(crate) fn tasks(&self) -> &crate::project_session::tasks::BackendTasks {
        &self.tasks
    }

    pub(crate) fn admit(
        &self,
    ) -> Result<crate::project_admission::ProjectActivity, crate::OperationError> {
        self.projects.admit(self.project.info().id())
    }

    /// Admit once and retain this project until the command completes, even if
    /// its HTTP caller disconnects. Long-lived work may retain the activity too.
    pub(crate) async fn execute<Output, Work>(
        &self,
        command: impl FnOnce(Self, crate::project_admission::ProjectActivity) -> Work + Send + 'static,
    ) -> Result<Output, crate::BackendCommandError>
    where
        Output: Send + 'static,
        Work: std::future::Future<Output = Result<Output, crate::BackendCommandError>> + Send,
    {
        let activity = self.admit()?;
        let state = self.clone();
        self.tasks
            .execute(async move {
                let result = command(state, activity.clone()).await;
                drop(activity);
                result
            })
            .await?
    }

    pub(crate) fn require_catalog_ready(&self) -> Result<(), crate::BackendCommandError> {
        match self.bootstrap().catalog_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    pub(crate) fn project(&self) -> &Arc<crate::project_session::ProjectSession> {
        &self.project
    }
    pub(crate) fn projects(&self) -> &Arc<crate::project_session::ProjectCoordinator> {
        &self.projects
    }
    pub(crate) fn backend_state(&self) -> &Arc<spiral_robotics_storage::BackendStateStore> {
        self.projects.backend_state()
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
    pub(crate) fn catalog_lease(&self) -> &Arc<CatalogMutationLease> {
        self.project.catalog_lease()
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        self.projects.hardware()
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        self.projects.cameras()
    }
    pub(crate) fn recording_session(&self) -> &Arc<RecordingSession> {
        self.project.recording()
    }
    pub(crate) fn video_encoder(&self) -> &VideoEncoderSettings {
        self.projects.video_encoder()
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
    pub(crate) fn bootstrap(&self) -> &ProjectReadiness {
        self.project.readiness()
    }
}
