//! Assistant tools own execution, provider contracts, availability, and receipts.

use super::super::WorkflowClient;
use super::{super::state::WorkflowController, ToolPublicText};
use crate::client::transport::HttpRequest;
use futures_util::future::LocalBoxFuture;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use spiral_robotics_common::workflow::{WorkflowAgentArtifactReceipt, WorkflowSpec};

mod author_sql;
mod dataset;
mod execution;
mod models;
mod relations;

use spiral_robotics_common::workflow::conversation::ProviderMessage;

pub(super) struct ToolContext {
    pub(super) expected: WorkflowSpec,
    pub(super) evidence: Vec<ProviderMessage>,
}
impl ToolContext {
    pub(super) fn new(expected: WorkflowSpec) -> Self {
        Self {
            expected,
            evidence: Vec::new(),
        }
    }
}

trait WorkflowTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    fn presentation(&self) -> ToolPresentation;
    fn available(&self, _workflow: &WorkflowSpec) -> bool {
        true
    }
    fn completion(&self, _output: &Value) -> ToolCompletion {
        ToolCompletion {
            summary: format!("{} complete", self.presentation().display_name()),
            receipt: None,
        }
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>>;

    fn definition(&self) -> Value {
        json!({"type":"function", "function": {
            "name": self.name(), "description": self.description(), "parameters": self.parameters()
        }})
    }
}

struct ToolInvocation<'a> {
    controller: WorkflowController,
    expected: &'a mut WorkflowSpec,
    arguments: &'a str,
    evidence: &'a mut Vec<ProviderMessage>,
}

impl ToolInvocation<'_> {
    fn arguments<T: DeserializeOwned>(&self) -> Result<T, String> {
        let arguments: Value = serde_json::from_str(self.arguments)
            .map_err(|error| format!("Tool arguments are malformed: {error}"))?;
        if !arguments.is_object() {
            return Err("Tool arguments are malformed: expected a JSON object.".into());
        }
        serde_json::from_value(arguments)
            .map_err(|error| format!("Tool arguments are malformed: {error}"))
    }
    async fn update_query(self, workflow: WorkflowSpec) -> Result<String, String> {
        if self.controller.spec() != *self.expected {
            return Err(
                "The draft changed while the assistant was working. Retry with the current SQL."
                    .into(),
            );
        }
        let validation = workflow.validate();
        if !validation.valid() {
            return Ok(json!({"ok":false, "validation":validation}).to_string());
        }
        let plan = WorkflowClient::current().plan(&workflow).await?;
        if let Some(error) = plan.error() {
            return Err(error.into());
        }
        self.controller
            .replace_authored_query(self.expected, workflow.clone())?;
        *self.expected = workflow.clone();
        Ok(json!({"ok":true, "workflow":workflow}).to_string())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

pub(super) struct WorkflowToolCatalog;

impl WorkflowToolCatalog {
    fn tools() -> [Box<dyn WorkflowTool>; 10] {
        [
            Box::new(dataset::InspectDataset),
            Box::new(relations::ListRelations),
            Box::new(relations::DescribeRelations),
            Box::new(models::ListModelCapabilities),
            Box::new(author_sql::SetWorkflow),
            Box::new(execution::RunWorkflow),
            Box::new(execution::InspectRun),
            Box::new(execution::InspectImages),
            Box::new(execution::ShowResults),
            Box::new(models::InspectFunctions),
        ]
    }
    fn find(name: &str) -> Option<Box<dyn WorkflowTool>> {
        Self::tools().into_iter().find(|tool| tool.name() == name)
    }
    pub(super) fn for_workflow(workflow: &WorkflowSpec) -> Vec<Value> {
        Self::tools()
            .into_iter()
            .filter(|tool| tool.available(workflow))
            .map(|tool| tool.definition())
            .collect()
    }
    pub(super) fn presentation(name: &str) -> ToolPresentation {
        Self::find(name).map_or_else(ToolPresentation::unknown, |tool| tool.presentation())
    }
    pub(super) fn completion(name: &str, output: &Value) -> ToolCompletion {
        Self::find(name).map_or_else(
            || ToolCompletion {
                summary: "Workflow tool complete".into(),
                receipt: None,
            },
            |tool| tool.completion(output),
        )
    }
    fn resolve(workflow: &WorkflowSpec, name: &str) -> Result<Box<dyn WorkflowTool>, String> {
        let tool = Self::find(name).ok_or_else(|| {
            format!(
                "Unknown Workflow tool {:?}. Use the available native workflow tools.",
                name
            )
        })?;
        if !tool.available(workflow) {
            return Err("Native episode authoring supports dataset inspection, model capabilities, and filter saves only. Computation is unavailable.".into());
        }
        Ok(tool)
    }

    pub(super) async fn execute(
        controller: WorkflowController,
        call: &spiral_robotics_common::workflow::conversation::ProviderToolCall,
        context: &mut ToolContext,
    ) -> Result<String, String> {
        let tool = Self::resolve(&context.expected, call.function().name())?;
        tool.call(ToolInvocation {
            controller,
            expected: &mut context.expected,
            evidence: &mut context.evidence,
            arguments: call.function().arguments(),
        })
        .await
    }
}

pub(super) struct ToolPresentation {
    running: &'static str,
    name: &'static str,
    failure: &'static str,
}

impl ToolPresentation {
    fn new(running: &'static str, name: &'static str, failure: &'static str) -> Self {
        Self {
            running,
            name,
            failure,
        }
    }
    fn unknown() -> Self {
        Self::new(
            "Using Workflow tool",
            "Workflow tool",
            "use the Workflow tool",
        )
    }
    pub(super) fn running_summary(&self) -> String {
        self.running.to_owned()
    }
    fn display_name(&self) -> &'static str {
        self.name
    }
    fn failure_action(&self) -> &'static str {
        self.failure
    }
}

pub(super) struct ToolCompletion {
    pub(super) summary: String,
    pub(super) receipt: Option<WorkflowAgentArtifactReceipt>,
}

impl ToolCompletion {
    pub(super) fn for_tool(name: &str, ok: bool, detail: &str) -> Self {
        let value = serde_json::from_str::<Value>(detail).unwrap_or(Value::Null);
        if !ok {
            return Self {
                summary: format!(
                    "Couldn’t {}",
                    WorkflowToolCatalog::presentation(name).failure_action()
                ),
                receipt: None,
            };
        }
        WorkflowToolCatalog::completion(name, &value)
    }

    fn query_updated(value: &Value) -> Self {
        let Some(workflow) = value.get("workflow") else {
            return ToolCompletion {
                summary: "SQL needs changes".to_owned(),
                receipt: None,
            };
        };
        let name = ToolPublicText::label(
            workflow
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Workflow"),
        );
        let revision = workflow.get("revision").and_then(Value::as_u64);
        let suffix = revision.map_or_else(String::new, |revision| format!(" · r{revision}"));
        ToolCompletion {
            summary: format!("Updated {name}{suffix}"),
            receipt: revision
                .map(|revision| WorkflowAgentArtifactReceipt::authored("View plan", revision)),
        }
    }

    fn count(value: &Value, field: &str, noun: &str) -> Self {
        let count = value
            .get(field)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        Self {
            summary: format!("Found {count} {noun}"),
            receipt: None,
        }
    }
}
