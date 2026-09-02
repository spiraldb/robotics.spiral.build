//! Workflow commands exposed to fullstack delivery adapters.

use std::sync::Arc;

use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};
use crate::http::routes::BootstrapState;
use crate::main_runtime::{MainRuntime, MainRuntimeFailure};
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;

/// Admission resources shared by Workflow commands.
pub(crate) struct WorkflowAdmission {
    catalog_lease: Arc<CatalogMutationLease>,
    bootstrap: BootstrapState,
}

impl WorkflowAdmission {
    pub(crate) fn new(catalog_lease: Arc<CatalogMutationLease>, bootstrap: BootstrapState) -> Self {
        Self {
            catalog_lease,
            bootstrap,
        }
    }
}

/// Narrow command facade shared with the Studio's server-function handlers.
#[derive(Clone)]
pub struct WorkflowCommands {
    main_runtime: MainRuntime,
    workflow: Arc<WorkflowManager>,
    agent: Arc<WorkflowAgentManager>,
    admission: Arc<WorkflowAdmission>,
}

/// One Workflow command failure whose backend message is shown verbatim by Studio.
#[derive(Debug)]
pub struct WorkflowFailure(String);

impl WorkflowCommands {
    pub(crate) fn new(
        main_runtime: MainRuntime,
        workflow: Arc<WorkflowManager>,
        agent: Arc<WorkflowAgentManager>,
        admission: WorkflowAdmission,
    ) -> Self {
        Self {
            main_runtime,
            workflow,
            agent,
            admission: Arc::new(admission),
        }
    }

    pub async fn snapshot(&self) -> Result<WorkflowSnapshot, WorkflowFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move {
                commands.require_catalog_ready()?;
                Ok(commands.workflow.state())
            })
            .await?
    }

    pub async fn command(
        &self,
        command: WorkflowCommand,
    ) -> Result<WorkflowSnapshot, WorkflowFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move {
                commands.require_catalog_ready()?;
                if matches!(&command, WorkflowCommand::Cancel { .. }) {
                    return commands.workflow.command(command).await.map_err(Into::into);
                }
                let token = commands
                    .admission
                    .catalog_lease
                    .try_acquire(CatalogLeaseOwner::WorkflowStart)
                    .ok_or_else(|| {
                        WorkflowFailure::conflict(
                            "Wait for the current recording or catalog change before changing Workflow",
                        )
                    })?;
                if commands.agent.is_active() {
                    let released = commands.admission.catalog_lease.release(token);
                    debug_assert!(
                        released,
                        "Workflow command releases its admission lease once"
                    );
                    return Err(WorkflowFailure::conflict(
                        "Wait for the active Assistant turn before changing Workflow manually",
                    ));
                }
                let result = commands.workflow.command(command).await.map_err(Into::into);
                let released = commands.admission.catalog_lease.release(token);
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
        self.main_runtime
            .execute(async move {
                commands.require_catalog_ready()?;
                Ok(commands.agent.state())
            })
            .await?
    }

    pub async fn agent_command(
        &self,
        command: WorkflowAgentCommand,
    ) -> Result<WorkflowAgentSnapshot, WorkflowFailure> {
        let commands = self.clone();
        self.main_runtime
            .execute(async move {
                commands.require_catalog_ready()?;
                if !matches!(&command, WorkflowAgentCommand::Cancel { .. })
                    && !commands
                        .admission
                        .bootstrap
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
                        commands
                            .admission
                            .catalog_lease
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
                let result = commands.agent.command(command).await.map_err(Into::into);
                if let Some(token) = token {
                    let released = commands.admission.catalog_lease.release(token);
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
        match self.admission.bootstrap.catalog_unavailable() {
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
