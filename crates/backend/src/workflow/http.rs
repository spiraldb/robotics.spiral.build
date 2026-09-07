//! HTTP endpoints for workflow.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};

pub(crate) struct WorkflowEndpoints;

impl WorkflowEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/workflow", Self::workflow_snapshot);
        routes.post("/workflow/commands", Self::run_workflow_command);
        routes.get("/workflow/assistant", Self::workflow_agent_snapshot);
        routes.post(
            "/workflow/assistant/commands",
            Self::run_workflow_agent_command,
        );
    }
    async fn workflow_snapshot(
        ScopedProject(commands): ScopedProject<crate::workflow::commands::WorkflowCommands>,
    ) -> Result<Json<WorkflowSnapshot>, HttpFailure> {
        let result = commands.snapshot().await?;
        Ok(Json(result))
    }
    async fn run_workflow_command(
        ScopedProject(commands): ScopedProject<crate::workflow::commands::WorkflowCommands>,
        ApiJson(command): ApiJson<WorkflowCommand>,
    ) -> Result<Json<WorkflowSnapshot>, HttpFailure> {
        let result = commands.command(command).await?;
        Ok(Json(result))
    }
    async fn workflow_agent_snapshot(
        ScopedProject(commands): ScopedProject<crate::workflow::commands::WorkflowCommands>,
    ) -> Result<Json<WorkflowAgentSnapshot>, HttpFailure> {
        let result = commands.agent_snapshot().await?;
        Ok(Json(result))
    }
    async fn run_workflow_agent_command(
        ScopedProject(commands): ScopedProject<crate::workflow::commands::WorkflowCommands>,
        ApiJson(command): ApiJson<WorkflowAgentCommand>,
    ) -> Result<Json<WorkflowAgentSnapshot>, HttpFailure> {
        let result = commands.agent_command(command).await?;
        Ok(Json(result))
    }
}
