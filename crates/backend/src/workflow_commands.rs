//! Workflow commands exposed to fullstack delivery adapters.

use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};

use crate::OperationError;
use crate::data::catalog_lease::CatalogLeaseOwner;
use crate::main_runtime::{MainRuntime, MainRuntimeFailure};

/// Narrow command facade shared with the Studio's server-function handlers.
#[derive(Clone)]
pub struct WorkflowCommands {
    main_runtime: MainRuntime,
    state: crate::http::routes::AppState,
}

/// One Workflow command failure whose backend message is shown verbatim by Studio.
#[derive(Debug)]
pub struct WorkflowFailure(String);

impl WorkflowCommands {
    pub(crate) fn new(main_runtime: MainRuntime, state: crate::http::routes::AppState) -> Self {
        Self {
            main_runtime,
            state,
        }
    }

    pub fn for_session(&self, session_id: &str) -> Result<Self, WorkflowFailure> {
        Ok(Self::new(
            self.main_runtime.clone(),
            self.state.for_session(session_id)?,
        ))
    }

    pub async fn snapshot(&self) -> Result<WorkflowSnapshot, WorkflowFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                Ok(commands.state.workflow().state())
            })
            .await?
    }

    pub async fn command(
        &self,
        command: WorkflowCommand,
    ) -> Result<WorkflowSnapshot, WorkflowFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                if matches!(&command, WorkflowCommand::Cancel { .. }) {
                    return commands.state.workflow().command(command).await.map_err(Into::into);
                }
                let token = commands.state.catalog_lease()
                    .try_acquire(CatalogLeaseOwner::WorkflowStart)
                    .ok_or_else(|| {
                        WorkflowFailure::conflict(
                            "Wait for the current recording or catalog change before changing Workflow",
                        )
                    })?;
                if commands.state.workflow_agent().is_active() {
                    let released = commands.state.catalog_lease().release(token);
                    debug_assert!(
                        released,
                        "Workflow command releases its admission lease once"
                    );
                    return Err(WorkflowFailure::conflict(
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

    pub async fn agent_snapshot(&self) -> Result<WorkflowAgentSnapshot, WorkflowFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                Ok(commands.state.workflow_agent().state())
            })
            .await?
    }

    pub async fn agent_command(
        &self,
        command: WorkflowAgentCommand,
    ) -> Result<WorkflowAgentSnapshot, WorkflowFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                commands.require_catalog_ready()?;
                if !matches!(&command, WorkflowAgentCommand::Cancel { .. })
                    && !commands.state.bootstrap()
                        .current()
                        .catalog_available()
                {
                    return Err(WorkflowFailure::conflict(
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
                                WorkflowFailure::conflict(
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

    fn require_catalog_ready(&self) -> Result<(), WorkflowFailure> {
        match self.state.bootstrap().catalog_unavailable() {
            Some(detail) => Err(WorkflowFailure(format!(
                "Studio catalog is not ready: {detail}"
            ))),
            None => Ok(()),
        }
    }
}

impl WorkflowFailure {
    fn conflict(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for WorkflowFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<OperationError> for WorkflowFailure {
    fn from(error: OperationError) -> Self {
        Self(error.to_string())
    }
}

impl From<MainRuntimeFailure> for WorkflowFailure {
    fn from(error: MainRuntimeFailure) -> Self {
        Self(error.to_string())
    }
}
