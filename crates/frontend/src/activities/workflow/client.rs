//! Planning and executing a workflow, and what Spiral can answer about. The
//! workflow document itself is listed, loaded, and saved by
//! `client::documents`.
use crate::activities::data::DatasetClient;
use crate::client::{
    project::ProjectClient,
    transport::{CommandError, HttpRequest},
};
use spiral_robotics_common::command_failure::CommandFailureCode;
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::workflow::{
    WorkflowSpec,
    document::{WorkflowPlan, WorkflowRun},
};

#[derive(Clone)]
pub(crate) struct WorkflowClient {
    project: ProjectClient,
}
impl WorkflowClient {
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    /// Spiral's structured plan for the SQL. Invalid SQL and a query Spiral
    /// rejects are failed plans; only a transport failure is an error.
    pub(super) async fn plan(&self, workflow: &WorkflowSpec) -> Result<WorkflowPlan, CommandError> {
        if let Some(error) = workflow.validate().errors().first() {
            return Ok(WorkflowPlan::failed(error.message()));
        }
        let statement = match workflow.plan_sql() {
            Ok(statement) => statement,
            Err(error) => return Ok(WorkflowPlan::failed(error)),
        };
        match DatasetClient::current()
            .run_spiral_query(statement, 1)
            .await
        {
            Ok(result) => Ok(WorkflowPlan::from_explain(&result)),
            Err(error)
                if matches!(
                    error.code(),
                    CommandFailureCode::External | CommandFailureCode::InvalidInput
                ) =>
            {
                Ok(WorkflowPlan::failed(error.to_string()))
            }
            Err(error) => Err(error),
        }
    }
    pub(super) async fn start(&self, workflow: &WorkflowSpec) -> Result<WorkflowRun, CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/workflow-runs").json(workflow))
            .await
    }
    /// The native AI functions this Spiral connection can call.
    pub(super) async fn capabilities(&self) -> Result<SpiralQuerySnapshot, CommandError> {
        DatasetClient::current()
            .run_spiral_query(
                "SELECT * FROM spql.meta.scalar_functions() WHERE function_set LIKE 'spql.ai.%' ORDER BY function_set, overload".into(),
                100,
            )
            .await
    }
    pub(super) async fn relations(&self) -> Result<RelationCatalog, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/relations"))
            .await
    }
}
