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
}
impl WorkflowDocument {
    pub fn new(workflow: WorkflowSpec) -> Self {
        let conversation = WorkflowConversation::new(workflow.id());
        Self {
            workflow,
            conversation,
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
    pub fn validate(&self) -> Result<(), String> {
        if self.workflow.id().is_empty() || self.workflow.id().len() > 128 {
            return Err("Workflow identity must contain 1–128 characters.".into());
        }
        if self.workflow.id() != self.conversation.workflow_id() {
            return Err("Conversation belongs to another workflow.".into());
        }
        self.conversation.validate()
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
