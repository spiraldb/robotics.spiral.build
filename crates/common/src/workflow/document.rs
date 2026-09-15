//! Saved authoring documents and stateless model requests.
use super::WorkflowSpec;
use super::conversation::{ProviderMessage, WorkflowConversation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDocument {
    workflow: WorkflowSpec,
    conversation: WorkflowConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    planned_sql: Option<WorkflowSqlPlan>,
}
impl WorkflowDocument {
    pub fn new(workflow: WorkflowSpec) -> Self {
        let conversation = WorkflowConversation::new(workflow.id());
        Self {
            workflow,
            conversation,
            planned_sql: None,
        }
    }
    pub fn with_conversation(mut self, conversation: WorkflowConversation) -> Self {
        self.conversation = conversation;
        self
    }
    pub fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }
    pub fn workflow_mut(&mut self) -> &mut WorkflowSpec {
        &mut self.workflow
    }
    pub fn conversation(&self) -> &WorkflowConversation {
        &self.conversation
    }
    pub fn with_plan(mut self, planned_sql: Option<WorkflowSqlPlan>) -> Self {
        self.planned_sql = planned_sql;
        self
    }
    pub fn planned_sql(&self) -> Option<&WorkflowSqlPlan> {
        self.planned_sql.as_ref()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.workflow.id().is_empty() || self.workflow.id().len() > 128 {
            return Err("Workflow identity must contain 1–128 characters.".into());
        }
        if self.workflow.id() != self.conversation.workflow_id() {
            return Err("Conversation belongs to another workflow.".into());
        }
        if let Some(planned) = &self.planned_sql {
            if planned.for_workflow(&self.workflow).is_none() {
                return Err("The saved plan belongs to different SQL.".into());
            }
            planned.plan.validate()?;
        }
        if let Some(comparison) = self.workflow.comparison() {
            comparison.validate()?;
        }
        self.conversation.validate()
    }
}

/// A display snapshot tied to the exact SQL that Spiral planned.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSqlPlan {
    sql: String,
    plan: WorkflowPlan,
}
impl WorkflowSqlPlan {
    pub fn new(workflow: &WorkflowSpec, plan: WorkflowPlan) -> Self {
        Self {
            sql: workflow
                .execution_sql()
                .unwrap_or_else(|_| workflow.sql().to_owned()),
            plan,
        }
    }
    pub fn for_workflow(&self, workflow: &WorkflowSpec) -> Option<&WorkflowPlan> {
        (workflow.execution_sql().is_ok_and(|sql| self.sql == sql)).then_some(&self.plan)
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    messages: Vec<ProviderMessage>,
    tools: Vec<Value>,
}
impl ModelRequest {
    pub fn new(messages: Vec<ProviderMessage>, tools: Vec<Value>) -> Self {
        Self { messages, tools }
    }
    pub fn messages(&self) -> &[ProviderMessage] {
        &self.messages
    }
    pub fn tools(&self) -> &[Value] {
        &self.tools
    }
}

pub use super::plan::WorkflowPlan;

/// Encoding version and payload of one workflow or conversation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowJson {
    version: u32,
    payload: String,
}

impl WorkflowJson {
    pub const fn new(version: u32, payload: String) -> Self {
        Self { version, payload }
    }
    pub const fn version(&self) -> u32 {
        self.version
    }
    pub fn payload(&self) -> &str {
        &self.payload
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum ModelEvent {
    Text(String),
    Complete(ProviderMessage),
    Error(String),
}

/// The exact submitted SQL travels with its execution observations.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowRun {
    workflow: WorkflowSpec,
    snapshot: super::WorkflowRunSnapshot,
}
impl WorkflowRun {
    pub fn new(workflow: WorkflowSpec, snapshot: super::WorkflowRunSnapshot) -> Self {
        Self { workflow, snapshot }
    }
    pub fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }
    pub fn snapshot(&self) -> &super::WorkflowRunSnapshot {
        &self.snapshot
    }
    pub fn snapshot_mut(&mut self) -> &mut super::WorkflowRunSnapshot {
        &mut self.snapshot
    }
}
