//! Workflow application operations.

use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};

use crate::BackendCommandError;
use crate::data::catalog_lease::CatalogLeaseOwner;

/// Project-bound operations; the context owns admission and backend task lifetime.
#[derive(Clone)]
pub(crate) struct WorkflowCommands {
    state: crate::project_session::context::ProjectContext,
}

impl WorkflowCommands {
    pub(crate) async fn snapshot(&self) -> Result<WorkflowSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .read(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                Ok(commands.state.workflow().state())
            })
            .await?
    }

    pub(crate) async fn command(
        &self,
        command: WorkflowCommand,
    ) -> Result<WorkflowSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state.tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                if matches!(&command, WorkflowCommand::Cancel { .. }) {
                    return commands.state.workflow().command(command).await.map_err(Into::into);
                }
                let token = commands.state.catalog_lease()
                    .try_acquire(CatalogLeaseOwner::WorkflowStart)
                    .ok_or_else(|| {
                        BackendCommandError::conflict(
                            "Wait for the current recording or catalog change before changing Workflow",
                        )
                    })?;
                if commands.state.workflow_agent().is_active() {
                    let released = commands.state.catalog_lease().release(token);
                    debug_assert!(
                        released,
                        "Workflow command releases its admission lease once"
                    );
                    return Err(BackendCommandError::conflict(
                        "Wait for the active Assistant turn before changing Workflow manually",
                    ));
                }
                let result = commands.state.workflow().command(command).await.map_err(Into::into);
                let released = commands.state.catalog_lease().release(token);
                debug_assert!(
                    released,
                    "Workflow command releases its admission lease once"
                );
                result
            })
            .await?
    }

    pub(crate) async fn agent_snapshot(
        &self,
    ) -> Result<WorkflowAgentSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .read(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                Ok(commands.state.workflow_agent().state())
            })
            .await?
    }

    pub(crate) async fn agent_command(
        &self,
        command: WorkflowAgentCommand,
    ) -> Result<WorkflowAgentSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state.tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                if !matches!(&command, WorkflowAgentCommand::Cancel { .. })
                    && !commands.state.bootstrap()
                        .current()
                        .catalog_available()
                {
                    return Err(BackendCommandError::conflict(
                        "Configure or reconnect Spiral before changing Assistant conversations.",
                    ));
                }
                let history_mutation = matches!(
                    &command,
                    WorkflowAgentCommand::CreateConversation { .. }
                        | WorkflowAgentCommand::ActivateConversation { .. }
                );
                let token = if history_mutation {
                    Some(
                        commands.state.catalog_lease()
                            .try_acquire(CatalogLeaseOwner::WorkflowAgentHistory)
                            .ok_or_else(|| {
                                BackendCommandError::conflict(
                                    "Wait for the current recording or catalog change before changing Assistant conversations",
                                )
                            })?,
                    )
                } else {
                    None
                };
                let result = commands.state.workflow_agent().command(command).await.map_err(Into::into);
                if let Some(token) = token {
                    let released = commands.state.catalog_lease().release(token);
                    debug_assert!(
                        released,
                        "Assistant command releases its history lease once"
                    );
                }
                result
            })
            .await?
    }

    fn require_catalog_ready(&self) -> Result<(), BackendCommandError> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}

impl From<crate::project_session::context::ProjectContext> for WorkflowCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}
