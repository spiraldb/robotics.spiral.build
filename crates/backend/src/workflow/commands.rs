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
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                commands.state.workflow_agent().ensure_loaded().await?;
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
                commands.state.workflow_agent().ensure_loaded().await?;
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
                let result = async {
                    // Resolve the conversation before changing the graph. A failed
                    // read leaves the selected graph and chat together.
                    if matches!(&command, WorkflowCommand::Create { .. } | WorkflowCommand::Select { .. })
                        && (commands.state.workflow().state().unsaved_changes()
                            || commands.state.workflow_agent().state().unsaved_changes())
                    {
                        return Err(BackendCommandError::conflict("Save or discard your changes before switching workflows."));
                    }
                    if let WorkflowCommand::Save { scope_id, expected_revision, workflow } = command {
                        commands.state.workflow().save(
                            super::manager::GraphEdit::new(scope_id, expected_revision, workflow),
                            &commands.state.workflow_agent().conversation(),
                        ).await?;
                        commands.state.workflow_agent().mark_saved();
                        return Ok(commands.state.workflow().state());
                    }
                    let conversation = match &command {
                        WorkflowCommand::Select { workflow_id, .. } => Some(commands.state.workflow_agent().conversation_for(workflow_id).await?),
                        WorkflowCommand::Discard { .. } => Some(commands.state.workflow_agent().conversation_for(commands.state.workflow().state().workflow().id()).await?),
                        _ => None,
                    };
                    let creating = matches!(&command, WorkflowCommand::Create { .. });
                    let state = commands.state.workflow().command(command).await?;
                    if let Some(conversation) = conversation {
                        commands.state.workflow_agent().select_conversation(conversation);
                    } else if creating {
                        commands.state.workflow_agent().select_conversation(
                            spiral_robotics_storage::workflow_assistant::WorkflowConversation::new(state.workflow().id())
                        );
                    }
                    Ok::<_, BackendCommandError>(commands.state.workflow().state())
                }.await;
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
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                commands.state.workflow_agent().ensure_loaded().await?;
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
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                commands.state.workflow_agent().ensure_loaded().await?;
                if !matches!(&command, WorkflowAgentCommand::Cancel { .. })
                    && !commands.state.bootstrap().current().catalog_available()
                {
                    return Err(BackendCommandError::conflict(
                        "Configure or reconnect Spiral before changing Assistant conversations.",
                    ));
                }
                commands
                    .state
                    .workflow_agent()
                    .command(command)
                    .await
                    .map_err(Into::into)
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
