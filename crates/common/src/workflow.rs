//! SQL workflow documents and independent query execution observations.
pub mod camera;
pub mod conversation;
pub mod document;
pub mod plan;

use crate::relations::SpiralQuerySnapshot;
use crate::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use serde::{Deserialize, Serialize};

pub const DEFAULT_WORKFLOW_ID: &str = "default";
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    id: String,
    name: String,
    revision: u64,
    sql: String,
}
impl Default for WorkflowSpec {
    fn default() -> Self {
        Self::new(DEFAULT_WORKFLOW_ID, "Untitled workflow")
    }
}
impl WorkflowSpec {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision: 0,
            sql: String::new(),
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_sql(mut self, sql: impl Into<String>) -> Self {
        self.sql = sql.into();
        self
    }
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub fn sql(&self) -> &str {
        &self.sql
    }
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
    pub fn set_sql(&mut self, sql: impl Into<String>) {
        self.sql = sql.into();
    }
    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
    /// A workflow always executes the SQL the user authored, unchanged.
    pub fn execution_sql(&self) -> Result<String, String> {
        Ok(self.sql.clone())
    }
    /// Spiral's structured plan for the executed SQL, as one read-only query.
    pub fn plan_sql(&self) -> Result<String, String> {
        Ok(format!("EXPLAIN (FORMAT JSON) {}", self.execution_sql()?))
    }
    pub fn accepts_output_from(&self, executed: &Self) -> bool {
        self.id == executed.id && self.sql == executed.sql
    }
    pub fn validate(&self) -> WorkflowValidationReport {
        let mut report = WorkflowValidationReport::default();
        if let Err(error) = ReadOnlyQuery::new(&self.sql, ReadOnlyQueryMode::Workflow).authorize() {
            report.errors.push(WorkflowValidationIssue::new(
                "invalid_query",
                &error.to_string(),
                "Write one SELECT or WITH query.",
            ));
        }
        report
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationReport {
    #[serde(default)]
    errors: Vec<WorkflowValidationIssue>,
    #[serde(default)]
    warnings: Vec<WorkflowValidationIssue>,
}

impl WorkflowValidationReport {
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}
impl WorkflowValidationIssue {
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowValidationIssue {
    code: String,
    message: String,
    recovery: String,
}

impl WorkflowValidationIssue {
    fn new(code: &str, message: &str, recovery: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            recovery: recovery.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct RunExecution {
    phase: WorkflowRunPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<SpiralQuerySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    result_stale: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkflowRunSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(flatten)]
    execution: RunExecution,
    #[serde(default)]
    started_ms: u64,
}

impl WorkflowRunSnapshot {
    /// Begin one submitted run; it runs until `finish` records how it ended.
    pub fn started(run_id: impl Into<String>) -> Self {
        let mut snapshot = Self {
            run_id: Some(run_id.into()),
            ..Self::default()
        };
        snapshot.execution.phase = WorkflowRunPhase::Running;
        snapshot
    }

    pub const fn with_started_ms(mut self, started_ms: u64) -> Self {
        self.started_ms = started_ms;
        self
    }

    pub fn finish(&mut self, result: Result<SpiralQuerySnapshot, String>) {
        match result {
            Ok(result) => {
                self.execution.phase = WorkflowRunPhase::Completed;
                self.execution.result = Some(result);
            }
            Err(error) => {
                self.execution.phase = WorkflowRunPhase::Failed;
                self.execution.error = Some(error);
            }
        }
    }

    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
    pub const fn phase(&self) -> WorkflowRunPhase {
        self.execution.phase
    }
    pub const fn result(&self) -> Option<&SpiralQuerySnapshot> {
        self.execution.result.as_ref()
    }
    pub fn error(&self) -> Option<&str> {
        self.execution.error.as_deref()
    }
    pub const fn result_stale(&self) -> bool {
        self.execution.result_stale
    }
    pub const fn started_ms(&self) -> u64 {
        self.started_ms
    }
    #[cfg(any(test, feature = "test-support"))]
    pub const fn set_phase(&mut self, phase: WorkflowRunPhase) {
        self.execution.phase = phase;
    }
    pub const fn set_result_stale(&mut self, result_stale: bool) {
        self.execution.result_stale = result_stale;
    }
}

/// Workflow runs share the one native run lifecycle.
pub use crate::runs::RunPhase as WorkflowRunPhase;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentRole {
    User,
    Assistant,
    Synthetic,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentMessagePhase {
    Streaming,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentToolPhase {
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentArtifactReceipt {
    label: String,
    revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentToolSummary {
    call_id: String,
    name: String,
    phase: WorkflowAgentToolPhase,
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<WorkflowAgentArtifactReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MessageIdentity {
    id: String,
    sequence: u64,
    role: WorkflowAgentRole,
    created_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MessageContent {
    phase: WorkflowAgentMessagePhase,
    text: String,
    tools: Vec<WorkflowAgentToolSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentMessage {
    #[serde(flatten)]
    identity: MessageIdentity,
    #[serde(flatten)]
    content: MessageContent,
}

impl WorkflowAgentMessage {
    pub fn new(id: impl Into<String>, role: WorkflowAgentRole) -> Self {
        Self {
            identity: MessageIdentity {
                id: id.into(),
                sequence: 0,
                role,
                created_ms: 0,
            },
            content: MessageContent {
                phase: WorkflowAgentMessagePhase::Streaming,
                text: String::new(),
                tools: Vec::new(),
                run_id: None,
            },
        }
    }

    pub const fn with_sequence(mut self, sequence: u64) -> Self {
        self.identity.sequence = sequence;
        self
    }

    pub const fn with_phase(mut self, phase: WorkflowAgentMessagePhase) -> Self {
        self.content.phase = phase;
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content.text = text.into();
        self
    }

    pub const fn with_created_ms(mut self, created_ms: u64) -> Self {
        self.identity.created_ms = created_ms;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub const fn role(&self) -> WorkflowAgentRole {
        self.identity.role
    }
    pub const fn phase(&self) -> WorkflowAgentMessagePhase {
        self.content.phase
    }
    pub fn text(&self) -> &str {
        &self.content.text
    }
    pub fn tools(&self) -> &[WorkflowAgentToolSummary] {
        &self.content.tools
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentMessagePhase) {
        self.content.phase = phase;
    }
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.content.text = text.into();
    }
    pub fn tools_mut(&mut self) -> &mut Vec<WorkflowAgentToolSummary> {
        &mut self.content.tools
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentPhase {
    #[default]
    Idle,
    Responding,
    Cancelling,
    Failed,
    Cancelled,
    Interrupted,
}

impl WorkflowAgentPhase {
    pub const fn active(self) -> bool {
        matches!(self, Self::Responding | Self::Cancelling)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionIdentity {
    workflow_id: String,
    conversation_id: String,
    revision: u64,
    updated_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionActivity {
    phase: WorkflowAgentPhase,
    messages: Vec<WorkflowAgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentSession {
    #[serde(flatten)]
    identity: SessionIdentity,
    #[serde(flatten)]
    activity: SessionActivity,
}

impl Default for WorkflowAgentSession {
    fn default() -> Self {
        Self::new(DEFAULT_WORKFLOW_ID, String::new())
    }
}

impl WorkflowAgentSession {
    fn new(workflow_id: impl Into<String>, conversation_id: impl Into<String>) -> Self {
        Self {
            identity: SessionIdentity {
                workflow_id: workflow_id.into(),
                conversation_id: conversation_id.into(),
                revision: 0,
                updated_ms: 0,
            },
            activity: SessionActivity {
                phase: WorkflowAgentPhase::Idle,
                messages: Vec::new(),
                active_turn_id: None,
                error: None,
            },
        }
    }

    pub fn workflow_id(&self) -> &str {
        &self.identity.workflow_id
    }
    fn conversation_id(&self) -> &str {
        &self.identity.conversation_id
    }
    const fn revision(&self) -> u64 {
        self.identity.revision
    }
    pub const fn phase(&self) -> WorkflowAgentPhase {
        self.activity.phase
    }
    pub fn messages(&self) -> &[WorkflowAgentMessage] {
        &self.activity.messages
    }
    pub fn error(&self) -> Option<&str> {
        self.activity.error.as_deref()
    }
    const fn set_revision(&mut self, revision: u64) {
        self.identity.revision = revision;
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentPhase) {
        self.activity.phase = phase;
    }
    pub fn messages_mut(&mut self) -> &mut Vec<WorkflowAgentMessage> {
        &mut self.activity.messages
    }
    pub fn set_active_turn_id(&mut self, turn_id: Option<String>) {
        self.activity.active_turn_id = turn_id;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.activity.error = error;
    }
    const fn set_updated_ms(&mut self, updated_ms: u64) {
        self.identity.updated_ms = updated_ms;
    }
    const fn updated_ms(&self) -> u64 {
        self.identity.updated_ms
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowSummary {
    id: String,
    name: String,
}

impl WorkflowSummary {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl WorkflowAgentArtifactReceipt {
    pub fn authored(label: impl Into<String>, revision: u64) -> Self {
        Self {
            label: label.into(),
            revision,
        }
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}
impl WorkflowAgentToolSummary {
    pub fn new(
        call_id: impl Into<String>,
        name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            phase: WorkflowAgentToolPhase::Running,
            summary: summary.into(),
            error: None,
            receipt: None,
        }
    }
    pub const fn set_phase(&mut self, phase: WorkflowAgentToolPhase) {
        self.phase = phase;
    }
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = summary.into();
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }
    pub fn set_receipt(&mut self, receipt: Option<WorkflowAgentArtifactReceipt>) {
        self.receipt = receipt;
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn phase(&self) -> WorkflowAgentToolPhase {
        self.phase
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub const fn receipt(&self) -> Option<&WorkflowAgentArtifactReceipt> {
        self.receipt.as_ref()
    }
}

impl WorkflowValidationReport {
    pub fn errors(&self) -> &[WorkflowValidationIssue] {
        &self.errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sql_is_the_only_executable_definition() {
        let workflow = WorkflowSpec::new("query", "Query").with_sql("SELECT 1");
        assert!(workflow.validate().valid());
        let mut json = serde_json::to_value(&workflow).unwrap();
        assert_eq!(
            serde_json::from_value::<WorkflowSpec>(json.clone()).unwrap(),
            workflow
        );
        json["nodes"] = serde_json::json!([]);
        assert!(serde_json::from_value::<WorkflowSpec>(json).is_err());
        assert!(
            !WorkflowSpec::new("query", "Query")
                .with_sql("DELETE FROM episodes")
                .validate()
                .valid()
        );
    }
}
