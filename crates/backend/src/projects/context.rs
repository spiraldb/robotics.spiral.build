//! What a request can reach: Studio-wide services, and the project it names.
use crate::account::{SpiralSession, SpiralSessions};
use crate::curation::manager::CurationManager;
use crate::data::lerobot_import::LeRobotImports;
use crate::projects::{OpenProject, ProjectRegistry};
use crate::training::manager::TrainingManager;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_storage::{ProviderKeys, StorageManager};
use std::sync::Arc;

/// Services shared by every request: the open projects, the browsers' Spiral
/// sessions, and the provider keys.
#[derive(Clone)]
pub(crate) struct StudioContext {
    projects: Arc<ProjectRegistry>,
    sessions: Arc<SpiralSessions>,
    keys: Arc<ProviderKeys>,
}

impl StudioContext {
    pub(crate) fn new(
        projects: Arc<ProjectRegistry>,
        sessions: Arc<SpiralSessions>,
        keys: Arc<ProviderKeys>,
    ) -> Self {
        Self {
            projects,
            sessions,
            keys,
        }
    }

    /// The project `key` names, as `session`'s user.
    pub(crate) async fn open(
        &self,
        session: &Arc<SpiralSession>,
        key: &ProjectKey,
    ) -> Result<ProjectContext, crate::OperationError> {
        let project = self.projects.open(session, key).await?;
        Ok(self.with_project(session.clone(), project))
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
    pub(crate) fn keys(&self) -> &ProviderKeys {
        &self.keys
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
    pub(crate) fn storage(&self) -> &Arc<StorageManager> {
        self.project.storage()
    }
    pub(crate) fn training(&self) -> &Arc<TrainingManager> {
        self.project.training()
    }
    pub(crate) fn curation(&self) -> &Arc<CurationManager> {
        self.project.curation()
    }
    pub(crate) fn lerobot_imports(&self) -> &Arc<LeRobotImports> {
        self.project.lerobot_imports()
    }
}
