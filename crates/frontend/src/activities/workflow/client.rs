//! Workflow requests and their query invalidation policy.
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};
use std::future::Future;

pub(crate) struct WorkflowClient {
    project: ProjectClient,
}
impl WorkflowClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn workflow_snapshot(
        &self,
    ) -> impl Future<Output = Result<WorkflowSnapshot, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/workflow"))
    }

    pub(crate) fn workflow_agent_snapshot(
        &self,
    ) -> impl Future<Output = Result<WorkflowAgentSnapshot, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/workflow/assistant"))
    }

    pub(crate) fn run_workflow_command(
        &self,
        command: WorkflowCommand,
    ) -> impl Future<Output = Result<WorkflowSnapshot, CommandError>> + use<> {
        self.project.mutate(
            &[Workflow, Assistant, Catalog],
            HttpRequest::new("POST", "/api/workflow/commands").json(&command),
        )
    }

    pub(crate) fn run_workflow_agent_command(
        &self,
        command: WorkflowAgentCommand,
    ) -> impl Future<Output = Result<WorkflowAgentSnapshot, CommandError>> + use<> {
        self.project.mutate(
            &[Assistant, Workflow, Catalog],
            HttpRequest::new("POST", "/api/workflow/assistant/commands").json(&command),
        )
    }
}
