//! Assistant tools own execution, provider contracts, availability, and receipts.

use super::{ToolContext, ToolPublicText, WorkflowAgentManager};
use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use spiral_robotics_common::workflow::{WorkflowAgentArtifactReceipt, WorkflowSpec};

mod dataset;
mod models;
mod relations;
mod save_graph;

trait WorkflowTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    fn presentation(&self) -> ToolPresentation;
    fn available(&self, workflow: &WorkflowSpec) -> bool {
        !workflow.uses_native_episodes()
    }
    fn completion(&self, _output: &Value) -> ToolCompletion {
        ToolCompletion {
            summary: format!("{} complete", self.presentation().display_name()),
            receipt: None,
        }
    }
    fn call<'a>(&'a self, invocation: ToolInvocation<'a>) -> BoxFuture<'a, Result<String, String>>;

    fn definition(&self) -> Value {
        json!({"type":"function", "function": {
            "name": self.name(), "description": self.description(), "parameters": self.parameters()
        }})
    }
}

struct ToolInvocation<'a> {
    agent: &'a WorkflowAgentManager,
    context: &'a mut ToolContext,
    arguments: &'a str,
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
    fn storage(&self) -> &spiral_robotics_storage::StorageManager {
        &self.agent.inner.services.storage
    }
    fn workflow(&self) -> WorkflowSpec {
        self.agent
            .inner
            .services
            .workflow
            .state()
            .workflow()
            .clone()
    }
    async fn save(self, workflow: WorkflowSpec) -> Result<String, String> {
        self.agent
            .save_authored_workflow(workflow, self.context)
            .await
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

pub(super) struct WorkflowToolCatalog;

impl WorkflowToolCatalog {
    fn tools() -> [Box<dyn WorkflowTool>; 7] {
        [
            Box::new(dataset::InspectDataset),
            Box::new(dataset::SetEpisodeFilter),
            Box::new(relations::ListRelations),
            Box::new(relations::DescribeRelations),
            Box::new(models::ListModelCapabilities),
            Box::new(save_graph::SetWorkflow),
            Box::new(relations::RunQuery),
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
        let tool = Self::find(name).ok_or_else(|| format!(
            "Unknown Workflow tool {:?}. The Assistant only authors workflows; run computation explicitly from the workflow controls.", name
        ))?;
        if !tool.available(workflow) {
            return Err("Native episode authoring supports dataset inspection, model capabilities, and filter saves only. Computation is unavailable.".into());
        }
        Ok(tool)
    }

    pub(super) async fn execute(
        agent: &WorkflowAgentManager,
        call: &spiral_robotics_storage::workflow_assistant::ProviderToolCall,
        context: &mut ToolContext,
    ) -> Result<String, String> {
        let tool = Self::resolve(
            agent.inner.services.workflow.state().workflow(),
            call.function().name(),
        )?;
        tool.call(ToolInvocation {
            agent,
            context,
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

    fn graph_saved(value: &Value) -> Self {
        let Some(workflow) = value.get("workflow") else {
            return ToolCompletion {
                summary: "Graph needs changes".to_owned(),
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
            summary: format!("Saved {name}{suffix}"),
            receipt: revision
                .map(|revision| WorkflowAgentArtifactReceipt::graph("View graph", revision)),
        }
    }

    fn query_preview(value: &Value) -> Self {
        let count = value
            .get("row_count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or_else(|| {
                value
                    .get("rows")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            });
        ToolCompletion {
            summary: format!(
                "Inspected {count} rows{}",
                if value.get("truncated").and_then(Value::as_bool) == Some(true) {
                    " · bounded preview"
                } else {
                    ""
                }
            ),
            receipt: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::catalog_lease::CatalogMutationLease;
    use crate::workflow::manager::WorkflowManager;
    use spiral_robotics_common::datasets::{DatasetSelection, DatasetSummary};
    use spiral_robotics_storage::workflow_assistant::ProviderToolCall;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    struct ToolFixture {
        agent: WorkflowAgentManager,
        context: ToolContext,
    }

    impl ToolFixture {
        fn new() -> Self {
            let settings = spiral_robotics_storage::SpiralConnectionSettings::new(
                "http://127.0.0.1:1".into(),
                "http://127.0.0.1:2".into(),
            )
            .unwrap();
            let database = Arc::new(spiral_robotics_storage::AdbcDatabase::new(settings.clone()));
            let project = spiral_robotics_storage::ProjectStore::new(
                settings,
                "tool_registry_test".into(),
                database,
            )
            .unwrap();
            let storage = Arc::new(spiral_robotics_storage::StorageManager::new(Arc::new(
                project,
            )));
            let workflow = Arc::new(WorkflowManager::new(storage.clone()));
            let saved = workflow.state();
            Self {
                context: ToolContext {
                    workflow_id: saved.workflow().id().to_owned(),
                    workflow_revision: saved.workflow().revision(),
                },
                agent: WorkflowAgentManager::new(
                    storage,
                    workflow,
                    Arc::new(CatalogMutationLease::default()),
                ),
            }
        }
        async fn call(&mut self, name: &str, arguments: &str) -> Result<String, String> {
            let call = ProviderToolCall::new("test-call", name, arguments);
            WorkflowToolCatalog::execute(&self.agent, &call, &mut self.context).await
        }
    }

    #[test]
    fn registry_dispatch_and_advertisement_share_the_same_availability_policy() {
        let relation = WorkflowSpec::new("relations", "Relations");
        let mut episodes = WorkflowSpec::new("episodes", "Episodes");
        episodes.select_dataset(DatasetSummary::new(
            DatasetSelection::new(vec!["dataset_demo".into()]).unwrap(),
            ("resource".into(), "version".into()),
        ));
        let tools = WorkflowToolCatalog::tools();
        let names = tools
            .iter()
            .map(|tool| tool.name())
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), tools.len(), "tool names must be unique");
        for workflow in [&relation, &episodes] {
            let advertised = WorkflowToolCatalog::for_workflow(workflow);
            for name in
                names
                    .iter()
                    .copied()
                    .chain(["run_workflow", "wait_for_run", "invented_tool"])
            {
                let listed = advertised
                    .iter()
                    .any(|tool| tool["function"]["name"] == name);
                assert_eq!(
                    WorkflowToolCatalog::resolve(workflow, name).is_ok(),
                    listed,
                    "{name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn concrete_tools_reject_bad_arguments_before_storage_or_graph_mutation() {
        let mut fixture = ToolFixture::new();
        let before = fixture.agent.inner.services.workflow.state();
        for (name, arguments) in [
            ("inspect_dataset", "[]"),
            ("set_episode_filter", r#"{"name":"Lighting"}"#),
            ("list_relations", r#"{"unexpected":true}"#),
            ("describe_relations", r#"{"relations":[42]}"#),
            ("list_model_capabilities", "null"),
            ("set_workflow", r#"{"workflow":[]}"#),
            ("run_query", r#"{"sql":"SELECT 1","max_rows":"many"}"#),
            (
                "run_query",
                r#"{"sql":"SELECT 1","max_rows":18446744073709551615}"#,
            ),
            ("run_query", r#"{"sql":"SELECT 1","max_rows":0}"#),
        ] {
            let error = fixture.call(name, arguments).await.unwrap_err();
            assert!(
                error.contains("arguments are malformed")
                    || error.contains("requires a workflow object")
                    || error.contains("max_rows must be"),
                "{name}: {error}"
            );
            assert_eq!(fixture.agent.inner.services.workflow.state(), before);
            assert!(!fixture.agent.inner.services.workflow.is_active());
        }
    }

    #[tokio::test]
    async fn registered_capability_tool_executes_its_implementation() {
        let mut fixture = ToolFixture::new();
        let output: Value =
            serde_json::from_str(&fixture.call("list_model_capabilities", "{}").await.unwrap())
                .unwrap();
        assert_eq!(output["models"][0]["id"], "siglip-base");
        assert_eq!(output["models"][0]["native_episode_execution"], false);
        assert_eq!(
            WorkflowToolCatalog::completion("list_model_capabilities", &output).summary,
            "Found 1 model capabilities"
        );
    }
}
