//! Workspaces: the projects Studio names and shares. Every signed-in user sees
//! the workspaces whose projects Spiral lets them open, and each browser works
//! in one. Spiral alone decides who may open, change, or delete which.

use std::collections::HashSet;
use std::sync::Arc;

use spiral_robotics_common::settings::SpiralSettings;
use spiral_robotics_common::workspace::{NewWorkspace, Workspace};

use super::ProjectRegistry;
use crate::OperationError;
use crate::account::SpiralSession;

impl ProjectRegistry {
    /// Every workspace `session`'s user can open, by name.
    pub(crate) async fn workspaces(
        &self,
        session: &SpiralSession,
    ) -> Result<Vec<Workspace>, OperationError> {
        let mut workspaces = self.openable_workspaces(session).await?;
        workspaces.sort_by_cached_key(|workspace| workspace.name().to_lowercase());
        Ok(workspaces)
    }

    /// Where a browser without a workspace starts: the oldest workspace its
    /// user can open.
    pub(super) async fn first_workspace(
        &self,
        session: &SpiralSession,
    ) -> Result<Option<Workspace>, OperationError> {
        Ok(self.openable_workspaces(session).await?.into_iter().next())
    }

    /// The workspaces, oldest first, whose projects `session`'s user can
    /// open. Spiral is asked only when there are workspaces to show.
    async fn openable_workspaces(
        &self,
        session: &SpiralSession,
    ) -> Result<Vec<Workspace>, OperationError> {
        let workspaces = self
            .workspaces
            .all()
            .await
            .map_err(OperationError::Internal)?;
        if workspaces.is_empty() {
            return Ok(workspaces);
        }
        let projects: HashSet<String> = session
            .projects()
            .await?
            .into_iter()
            .map(|project| project.id().to_owned())
            .collect();
        Ok(workspaces
            .into_iter()
            .filter(|workspace| projects.contains(workspace.project_id()))
            .collect())
    }

    /// A workspace on the existing project `request` names, which its user
    /// must be able to open.
    pub(crate) async fn create_workspace(
        &self,
        session: &SpiralSession,
        request: &NewWorkspace,
    ) -> Result<Workspace, OperationError> {
        let name =
            Workspace::validated_name(request.name()).map_err(OperationError::InvalidInput)?;
        let project_id = match request.project_id() {
            Some(project_id) => {
                self.endpoints
                    .clone()
                    .with_project(project_id)
                    .map_err(OperationError::InvalidInput)?;
                if let Some(existing) = self.find_workspace(project_id).await? {
                    return Err(Self::taken(&existing));
                }
                session.admit(project_id).await?;
                project_id.to_owned()
            }
            None => {
                return Err(OperationError::InvalidInput(
                    "Choose an existing Spiral project. Create new projects in Spiral Studio."
                        .to_owned(),
                ));
            }
        };
        let workspace = Workspace::new(project_id, name);
        if !self
            .workspaces
            .insert(&workspace)
            .await
            .map_err(OperationError::Internal)?
        {
            let existing = self.workspace(workspace.project_id()).await?;
            return Err(Self::taken(&existing));
        }
        Ok(workspace)
    }

    fn taken(existing: &Workspace) -> OperationError {
        OperationError::conflict(format!(
            "The project {} is already the workspace “{}”.",
            existing.project_id(),
            existing.name()
        ))
    }

    /// Work in the workspace on `project_id` from now on, which `session`'s
    /// user must be able to open.
    pub(crate) async fn choose_workspace(
        &self,
        session: &Arc<SpiralSession>,
        project_id: &str,
    ) -> Result<SpiralSettings, OperationError> {
        let connection = self
            .endpoints
            .clone()
            .with_project(project_id)
            .map_err(OperationError::InvalidInput)?;
        if self.find(session, &connection).is_none() {
            self.workspace(project_id).await?;
            session.admit(project_id).await?;
            let candidate = self.factory.create(connection, session.access())?;
            self.adopt(session, candidate);
        }
        session.choose_project(project_id).await?;
        Ok(self.present(project_id))
    }

    /// Forget the workspace on `project_id` and every record Studio keeps for
    /// it, and close it in every browser. Its Spiral project stays.
    pub(crate) async fn delete_workspace(
        &self,
        session: &SpiralSession,
        project_id: &str,
    ) -> Result<(), OperationError> {
        let connection = self
            .endpoints
            .clone()
            .with_project(project_id)
            .map_err(OperationError::InvalidInput)?;
        self.workspace(project_id).await?;
        session.admit(project_id).await?;
        let open: Vec<_> = self
            .opened()
            .into_iter()
            .filter(|(_, project)| project.connection() == connection)
            .collect();
        for (_, project) in open {
            if project.busy().await? {
                return Err(OperationError::conflict(
                    "Stop training before deleting this workspace.",
                ));
            }
        }
        self.workspaces
            .delete(project_id)
            .await
            .map_err(OperationError::Internal)?;
        Self::shut_down(self.retire(|opened| opened.project.connection() == connection));
        Ok(())
    }

    pub(super) async fn rename_workspace(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<(), OperationError> {
        let name = Workspace::validated_name(name).map_err(OperationError::InvalidInput)?;
        if self
            .workspaces
            .rename(project_id, &name)
            .await
            .map_err(OperationError::Internal)?
        {
            Ok(())
        } else {
            Err(Self::gone())
        }
    }

    /// The workspace on `project_id`; a request still naming one deleted
    /// since is stale.
    pub(super) async fn workspace(&self, project_id: &str) -> Result<Workspace, OperationError> {
        self.find_workspace(project_id)
            .await?
            .ok_or_else(Self::gone)
    }

    pub(super) async fn find_workspace(
        &self,
        project_id: &str,
    ) -> Result<Option<Workspace>, OperationError> {
        self.workspaces
            .find(project_id)
            .await
            .map_err(OperationError::Internal)
    }

    fn gone() -> OperationError {
        OperationError::StaleProject(
            "This workspace was deleted. Choose another workspace.".to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::project::ProjectKey;
    use spiral_robotics_common::settings::{RobotConnectorSettings, SettingsUpdate};
    use spiral_robotics_common::workspace::{NewWorkspace, Workspace};

    use crate::OperationError;
    use crate::projects::tests::{ENDPOINT, ProjectFixture};

    fn existing(name: &str, project_id: &str) -> NewWorkspace {
        NewWorkspace::new(name, Some(project_id.to_owned()))
    }

    #[tokio::test]
    async fn workspaces_are_shared_named_and_on_projects_spiral_shows() {
        let fixture = ProjectFixture::new().await;
        let session = fixture.session().await;
        let registry = &fixture.projects;
        assert!(registry.workspaces(&session).await.unwrap().is_empty());
        let created = registry
            .create_workspace(&session, &existing(" Lab arms ", "project_a"))
            .await
            .unwrap();
        assert_eq!(created, Workspace::new("project_a", "Lab arms"));
        registry
            .create_workspace(&session, &existing("bench", "project_b"))
            .await
            .unwrap();
        for (request, refused) in [
            (NewWorkspace::new("New project", None), "invalid"),
            (existing("Again", "project_b"), "taken"),
            (existing("Hidden", "forbidden_arms"), "hidden"),
            (existing("Escape", "../v1/organizations"), "invalid"),
            (existing("  ", "project_a"), "invalid"),
        ] {
            let result = registry.create_workspace(&session, &request).await;
            assert!(
                matches!(
                    (refused, &result),
                    ("taken", Err(OperationError::Conflict(_)))
                        | ("hidden", Err(OperationError::NotFound(_)))
                        | ("invalid", Err(OperationError::InvalidInput(_)))
                ),
                "{refused}: {result:?}"
            );
        }
        // A workspace on a project this user cannot open is someone else's.
        let theirs = Workspace::new("forbidden_arms", "Theirs");
        assert!(registry.workspaces.insert(&theirs).await.unwrap());
        // Every browser sees the same workspaces, by name.
        let elsewhere = fixture.another_session().await;
        assert_eq!(
            registry.workspaces(&elsewhere).await.unwrap(),
            [
                Workspace::new("project_b", "bench"),
                Workspace::new("project_a", "Lab arms")
            ]
        );
    }

    #[tokio::test]
    async fn a_browser_starts_in_the_oldest_workspace_and_keeps_its_choice() {
        let fixture = ProjectFixture::new().await;
        let session = fixture.session().await;
        let registry = &fixture.projects;
        let working_in = || async {
            registry
                .session_settings(&session)
                .await
                .unwrap()
                .project_id()
                .to_owned()
        };
        assert_eq!(working_in().await, "");
        let theirs = Workspace::new("forbidden_arms", "Theirs");
        assert!(registry.workspaces.insert(&theirs).await.unwrap());
        assert_eq!(working_in().await, "");
        for project in ["project_b", "project_a"] {
            registry
                .create_workspace(&session, &existing(project, project))
                .await
                .unwrap();
        }
        assert_eq!(working_in().await, "project_b");
        let chosen = registry
            .choose_workspace(&session, "project_a")
            .await
            .unwrap();
        assert_eq!(chosen.project_id(), "project_a");
        assert_eq!(working_in().await, "project_a");
        // Another browser of the same user still starts in the oldest.
        let elsewhere = fixture.another_session().await;
        assert_eq!(
            registry
                .session_settings(&elsewhere)
                .await
                .unwrap()
                .project_id(),
            "project_b"
        );
        registry
            .delete_workspace(&session, "project_a")
            .await
            .unwrap();
        assert_eq!(working_in().await, "project_b");
        assert!(matches!(
            registry.choose_workspace(&session, "project_a").await,
            Err(OperationError::StaleProject(_))
        ));
    }

    #[tokio::test]
    async fn deleting_a_workspace_closes_it_in_every_browser_and_forgets_its_settings() {
        let fixture = ProjectFixture::new().await;
        let session = fixture.session().await;
        let registry = &fixture.projects;
        let project = fixture.open("project_a").await;
        let renamed = registry
            .update_settings(&project, SettingsUpdate::new("Lab arms"))
            .await
            .unwrap();
        assert_eq!(renamed.name(), "Lab arms");
        let connected = registry
            .set_robot_connector(
                &project,
                RobotConnectorSettings::new("http://lab-arms.example:7070/"),
            )
            .await
            .unwrap();
        assert_eq!(
            connected.robot_connector().address(),
            "http://lab-arms.example:7070"
        );
        assert_eq!(connected.name(), "Lab arms");
        assert!(matches!(
            registry
                .set_robot_connector(&project, RobotConnectorSettings::new("lab-arms:7070"))
                .await,
            Err(OperationError::InvalidInput(_))
        ));
        let unchosen = fixture.open("").await;
        assert!(matches!(
            registry
                .set_robot_connector(&unchosen, RobotConnectorSettings::default())
                .await,
            Err(OperationError::InvalidInput(_))
        ));

        let elsewhere = fixture.another_session().await;
        registry
            .open(&elsewhere, &ProjectKey::new(ENDPOINT, "project_a"))
            .await
            .unwrap();
        registry
            .delete_workspace(&session, "project_a")
            .await
            .unwrap();
        assert!(
            registry
                .opened()
                .iter()
                .all(|(_, opened)| opened.connection().project_id() != Some("project_a"))
        );
        for browser in [&session, &elsewhere] {
            assert!(matches!(
                registry
                    .open(browser, &ProjectKey::new(ENDPOINT, "project_a"))
                    .await,
                Err(OperationError::StaleProject(_))
            ));
        }
        assert!(matches!(
            registry.delete_workspace(&session, "project_a").await,
            Err(OperationError::StaleProject(_))
        ));
        // The project's Studio records went with it: made a workspace again,
        // it starts fresh.
        let again = fixture.open("project_a").await;
        let settings = registry.settings(&again).await.unwrap();
        assert_eq!(settings.name(), "project_a");
        assert_eq!(
            settings.robot_connector(),
            &RobotConnectorSettings::default()
        );
    }
}
