//! Backend-owned Workflow agent, tool loop, ephemeral session, and query state.

mod tools;
use tools::{ToolCompletion, WorkflowToolCatalog};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};
use crate::sync::MutexRecovery;
use serde_json::{Value, json};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_common::workflow::{
    WorkflowAgentCommand, WorkflowAgentMessage, WorkflowAgentMessagePhase, WorkflowAgentPhase,
    WorkflowAgentReadiness, WorkflowAgentRole, WorkflowAgentSession, WorkflowAgentSnapshot,
    WorkflowAgentToolPhase, WorkflowAgentToolSummary, WorkflowCommand, WorkflowNodeConfig,
    WorkflowSpec,
};

use crate::workflow::manager::WorkflowManager;
use crate::workflow::provider::{
    OpenAiProvider, ProviderCompletion, ProviderConfig, ProviderResponse,
};
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::workflow_assistant::{
    ProviderMessage, ProviderMessageHistory, ProviderToolCall, WorkflowConversation,
};
use spiral_robotics_storage::workflows::MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION;

const MAX_PUBLIC_MESSAGES: usize = MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION;
const MAX_WORKFLOW_AGENT_INPUT_CHARS: usize = 8_000;
const MAX_WORKFLOW_AGENT_QUERY_ROWS: usize = 50;
const MAX_TOOL_ITERATIONS: usize = 16;
const MAX_TOOL_CALLS: usize = 48;
const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_CELL_CHARS: usize = 512;
const MAX_QUERY_PROMPT_BYTES: usize = 24 * 1024;
const ASSISTANT_READY_DETAIL: &str = "Ready for relation-first Workflow turns.";
const ASSISTANT_UNAVAILABLE_DETAIL: &str =
    "Assistant is temporarily unavailable. Retry in a moment.";

#[derive(Clone)]
pub(crate) struct WorkflowAgentManager {
    inner: Arc<WorkflowAgentInner>,
}

struct WorkflowAgentInner {
    services: WorkflowAgentServices,
    readiness: WorkflowAgentReadiness,
    scope_id: Mutex<String>,
    conversation: Mutex<WorkflowConversation>,
    turn: AgentTurnState,
}

struct AgentTurnState {
    loaded: tokio::sync::OnceCell<()>,
    active: Mutex<Option<ActiveTurn>>,
    mutation: tokio::sync::Mutex<()>,
    saved_revision: Mutex<u64>,
}

struct WorkflowAgentServices {
    storage: Arc<StorageManager>,
    workflow: Arc<WorkflowManager>,
    catalog_lease: Arc<CatalogMutationLease>,
    provider: Option<OpenAiProvider>,
}

struct ActiveTurn {
    conversation_id: String,
    turn_id: String,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

trait ConversationUpdates {
    fn with_completed_tool(
        &self,
        identity: TurnIdentity<'_>,
        completion: ToolRunCompletion<'_>,
    ) -> Result<Self, String>
    where
        Self: Sized;

    fn with_provider_message(
        &self,
        identity: TurnIdentity<'_>,
        message: ProviderMessage,
    ) -> Result<Self, String>
    where
        Self: Sized;
}
impl ConversationUpdates for WorkflowConversation {
    fn with_completed_tool(
        &self,
        identity: TurnIdentity<'_>,
        completion: ToolRunCompletion<'_>,
    ) -> Result<Self, String> {
        let TurnIdentity {
            conversation_id,
            turn_id,
        } = identity;
        let ToolRunCompletion { call, ok, detail } = completion;
        let mut candidate = self.clone();
        candidate.ensure_turn(conversation_id, turn_id)?;
        let message = candidate
            .session_mut()
            .current_assistant_mut()
            .ok_or_else(|| "Assistant message disappeared during tool execution.".to_owned())?;
        let summary = message
            .tools_mut()
            .iter_mut()
            .find(|summary| summary.call_id() == call.id())
            .ok_or_else(|| "Assistant tool summary disappeared.".to_owned())?;
        let detail_value = serde_json::from_str::<Value>(detail).unwrap_or(Value::Null);
        let effective_ok = ok
            && detail_value
                .get("ok")
                .and_then(Value::as_bool)
                .unwrap_or(true);
        let phase = if effective_ok {
            WorkflowAgentToolPhase::Complete
        } else {
            WorkflowAgentToolPhase::Failed
        };
        let completion = ToolCompletion::for_tool(call.function().name(), effective_ok, detail);
        summary.set_phase(phase);
        summary.set_summary(completion.summary);
        summary.set_error(
            (!effective_ok)
                .then(|| ToolPublicText::detail(detail))
                .flatten(),
        );
        summary.set_receipt(completion.receipt);
        let conversation = &mut candidate;
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::tool(
                call.id().to_owned(),
                detail.to_owned(),
            ));
        conversation
            .provider_messages_mut()
            .trim_to_provider_bound();
        candidate.advance()?;
        Ok(candidate)
    }

    fn with_provider_message(
        &self,
        identity: TurnIdentity<'_>,
        message: ProviderMessage,
    ) -> Result<Self, String> {
        let TurnIdentity {
            conversation_id,
            turn_id,
        } = identity;
        self.ensure_turn(conversation_id, turn_id)?;
        let mut candidate = self.clone();
        let conversation = &mut candidate;
        conversation.provider_messages_mut().push(message);
        conversation
            .provider_messages_mut()
            .trim_to_provider_bound();
        Ok(candidate)
    }
}

struct PublicMessageCapacity(usize);

impl PublicMessageCapacity {
    fn new(current: usize) -> Self {
        Self(current)
    }

    fn ensure_additional(&self, additional: usize) -> Result<(), String> {
        if self
            .0
            .checked_add(additional)
            .is_none_or(|total| total > MAX_PUBLIC_MESSAGES)
        {
            return Err(format!(
                "This conversation reached its {MAX_PUBLIC_MESSAGES}-message history limit. Start a new conversation; this history will remain available."
            ));
        }
        Ok(())
    }
}

struct AgentIdentifier;

impl AgentIdentifier {
    fn next(prefix: &str) -> String {
        format!("{prefix}-{}", uuid::Uuid::new_v4())
    }
}

struct AgentClock;

impl AgentClock {
    fn now_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

struct ProviderReadiness;

impl ProviderReadiness {
    fn public(ready: bool) -> WorkflowAgentReadiness {
        if ready {
            WorkflowAgentReadiness::ready(ASSISTANT_READY_DETAIL)
        } else {
            WorkflowAgentReadiness::error(ASSISTANT_UNAVAILABLE_DETAIL)
        }
    }
}

trait WorkflowAgentSessionExt {
    fn next_message_sequence(&self) -> u64;
    fn current_assistant_mut(&mut self) -> Option<&mut WorkflowAgentMessage>;
}

impl WorkflowAgentSessionExt for WorkflowAgentSession {
    fn next_message_sequence(&self) -> u64 {
        self.messages()
            .last()
            .map_or(1, |message| message.sequence().saturating_add(1))
    }

    fn current_assistant_mut(&mut self) -> Option<&mut WorkflowAgentMessage> {
        self.messages_mut()
            .iter_mut()
            .rev()
            .find(|message| message.role() == WorkflowAgentRole::Assistant)
    }
}

trait UnexecutedProviderMessages {
    fn append_unexecuted_tool_outputs(&mut self, unexecuted: UnexecutedTools<'_>);
}
impl UnexecutedProviderMessages for Vec<ProviderMessage> {
    fn append_unexecuted_tool_outputs(&mut self, unexecuted: UnexecutedTools<'_>) {
        let output =
            ToolOutput::bounded_text(json!({"ok": false, "error": unexecuted.detail}).to_string());
        self.extend(
            unexecuted
                .calls
                .iter()
                .map(|call| ProviderMessage::tool(call.id().to_owned(), output.clone())),
        );
    }
}

struct ToolContext {
    workflow_id: String,
    workflow_revision: u64,
}

#[derive(Clone, Copy)]
struct TurnIdentity<'a> {
    conversation_id: &'a str,
    turn_id: &'a str,
}

impl<'a> TurnIdentity<'a> {
    fn new(conversation_id: &'a str, turn_id: &'a str) -> Self {
        Self {
            conversation_id,
            turn_id,
        }
    }
}

struct TurnStart {
    scope_id: String,
    conversation_id: String,
    text: String,
    expected_revision: u64,
}

struct TurnCancellation {
    scope_id: String,
    conversation_id: String,
    turn_id: String,
}

struct TurnTask {
    conversation_id: String,
    turn_id: String,
    cancel: CancellationToken,
}

impl TurnTask {
    fn new(conversation_id: String, turn_id: String) -> Self {
        Self {
            conversation_id,
            turn_id,
            cancel: CancellationToken::new(),
        }
    }

    fn with_runtime(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }
}

struct ProviderLoop<'a> {
    identity: TurnIdentity<'a>,
    cancel: CancellationToken,
    context: ToolContext,
    budget: TurnBudget,
}

impl<'a> ProviderLoop<'a> {
    fn new(task: &'a TurnTask, workflow_revision: u64) -> Self {
        Self {
            identity: TurnIdentity::new(&task.conversation_id, &task.turn_id),
            cancel: task.cancel.clone(),
            context: ToolContext {
                workflow_id: String::new(),
                workflow_revision,
            },
            budget: TurnBudget::default(),
        }
    }
}

struct ToolOutput;

impl ToolOutput {
    fn bounded_text(output: String) -> String {
        if output.len() <= MAX_TOOL_OUTPUT_BYTES {
            output
        } else {
            json!({
                "ok": false,
                "error": "Tool output exceeded the bounded prompt payload limit.",
                "truncated": true,
            })
            .to_string()
        }
    }
}

struct UnexecutedTools<'a> {
    calls: &'a [ProviderToolCall],
    detail: &'a str,
}

impl<'a> UnexecutedTools<'a> {
    fn new(calls: &'a [ProviderToolCall], detail: &'a str) -> Self {
        Self { calls, detail }
    }
}

struct ToolRunCompletion<'a> {
    call: &'a ProviderToolCall,
    ok: bool,
    detail: &'a str,
}

impl<'a> ToolRunCompletion<'a> {
    fn new(call: &'a ProviderToolCall, ok: bool, detail: &'a str) -> Self {
        Self { call, ok, detail }
    }
}

struct TurnFinish<'a> {
    identity: TurnIdentity<'a>,
    phase: WorkflowAgentPhase,
    error: Option<String>,
    cancelled: bool,
}

struct TurnFailureDetail<'a> {
    error: &'a str,
    cancelled: bool,
}

impl<'a> TurnFailureDetail<'a> {
    fn new(error: &'a str, cancelled: bool) -> Self {
        Self { error, cancelled }
    }
}

impl<'a> TurnFinish<'a> {
    fn new(identity: TurnIdentity<'a>, phase: WorkflowAgentPhase) -> Self {
        Self {
            identity,
            phase,
            error: None,
            cancelled: false,
        }
    }

    fn with_error(mut self, error: Option<String>, cancelled: bool) -> Self {
        self.error = error;
        self.cancelled = cancelled;
        self
    }
}

#[derive(Default)]
struct TurnBudget {
    provider_iterations: usize,
    tool_calls: usize,
}

impl TurnBudget {
    fn admit_provider_iteration(&mut self) -> Result<(), String> {
        if self.provider_iterations >= MAX_TOOL_ITERATIONS {
            return Err("The Assistant exceeded the bounded tool-loop iteration limit.".to_owned());
        }
        self.provider_iterations += 1;
        Ok(())
    }

    fn admit_tool_calls(&mut self, count: usize) -> Result<(), String> {
        self.tool_calls = self.tool_calls.saturating_add(count);
        if self.tool_calls > MAX_TOOL_CALLS {
            return Err("The Assistant exceeded the bounded tool-call limit.".to_owned());
        }
        Ok(())
    }

    fn admit_provider_response(
        &mut self,
        response: &ProviderResponse,
    ) -> Result<ProviderMessage, String> {
        self.admit_tool_calls(response.tool_calls().len())?;
        Ok(ProviderMessage::assistant(
            (!response.content().is_empty()).then(|| response.content().to_owned()),
            response.tool_calls().to_vec(),
        ))
    }
}

impl WorkflowAgentManager {
    pub(crate) fn new(
        storage: Arc<StorageManager>,
        workflow: Arc<WorkflowManager>,
        catalog_lease: Arc<CatalogMutationLease>,
    ) -> Self {
        let configured = ProviderConfig::from_env();
        let (provider, readiness) = match configured {
            Ok(Some(config)) => match OpenAiProvider::new(config) {
                Ok(provider) => (Some(provider), ProviderReadiness::public(true)),
                Err(error) => {
                    tracing::error!(error = %error, "Workflow Assistant provider client initialization failed");
                    (None, ProviderReadiness::public(false))
                }
            },
            Ok(None) => {
                tracing::warn!(
                    "Workflow Assistant provider credential was not found in the process environment or repository-local credential file"
                );
                (None, ProviderReadiness::public(false))
            }
            Err(error) => {
                tracing::error!(error = %error, "Workflow Assistant provider configuration failed");
                (None, ProviderReadiness::public(false))
            }
        };
        Self {
            inner: Arc::new(WorkflowAgentInner {
                services: WorkflowAgentServices {
                    storage,
                    workflow,
                    catalog_lease,
                    provider,
                },
                readiness,
                scope_id: Mutex::new(AgentIdentifier::next("agent-scope")),
                conversation: Mutex::new(WorkflowConversation::new("unloaded")),
                turn: AgentTurnState {
                    loaded: tokio::sync::OnceCell::new(),
                    active: Mutex::new(None),
                    mutation: tokio::sync::Mutex::new(()),
                    saved_revision: Mutex::new(0),
                },
            }),
        }
    }

    pub(crate) async fn ensure_loaded(&self) -> Result<(), String> {
        self.inner
            .turn
            .loaded
            .get_or_try_init(|| async {
                self.inner
                    .services
                    .workflow
                    .ensure_loaded()
                    .await
                    .map_err(|error| error.to_string())?;
                self.prepare_from_storage().await?;
                Ok::<(), String>(())
            })
            .await
            .map(|_| ())
    }

    /// Candidate preparation validates history without rewriting it or starting a turn.
    pub(crate) async fn prepare_from_storage(&self) -> Result<(), String> {
        let workflow_id = self
            .inner
            .services
            .workflow
            .state()
            .workflow()
            .id()
            .to_owned();
        let stored = self
            .inner
            .services
            .storage
            .load_workflow_conversation(&workflow_id)
            .await?;
        let mut conversation = match stored.as_ref() {
            Some(stored) => WorkflowConversation::load(stored, &workflow_id)?,
            None => WorkflowConversation::new(&workflow_id),
        };
        *self.inner.turn.saved_revision.recover_lock() = conversation.revision();
        conversation.interrupt()?;
        *self.inner.conversation.recover_lock() = conversation;
        *self.inner.scope_id.recover_lock() = AgentIdentifier::next("agent-scope");
        Ok(())
    }

    pub(crate) async fn conversation_for(
        &self,
        workflow_id: &str,
    ) -> Result<WorkflowConversation, String> {
        let stored = self
            .inner
            .services
            .storage
            .load_workflow_conversation(workflow_id)
            .await?;
        let mut conversation = match stored {
            Some(stored) => WorkflowConversation::load(&stored, workflow_id)?,
            None => WorkflowConversation::new(workflow_id),
        };
        conversation.interrupt()?;
        Ok(conversation)
    }

    pub(crate) fn select_conversation(&self, conversation: WorkflowConversation) {
        debug_assert!(!self.is_active());
        *self.inner.turn.saved_revision.recover_lock() = conversation.revision();
        *self.inner.conversation.recover_lock() = conversation;
        *self.inner.scope_id.recover_lock() = AgentIdentifier::next("agent-scope");
    }

    pub(crate) fn conversation(&self) -> WorkflowConversation {
        self.inner.conversation.recover_lock().clone()
    }

    pub(crate) fn mark_saved(&self) {
        let revision = self.inner.conversation.recover_lock().revision();
        *self.inner.turn.saved_revision.recover_lock() = revision;
    }

    pub(crate) fn state(&self) -> WorkflowAgentSnapshot {
        let conversation = self.inner.conversation.recover_lock();
        self.state_for(&conversation)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.turn.active.recover_lock().is_some()
    }

    pub(crate) async fn command(
        &self,
        command: WorkflowAgentCommand,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        match command {
            WorkflowAgentCommand::Turn {
                scope_id,
                conversation_id,
                text,
                expected_revision,
            } => {
                self.start_turn(TurnStart {
                    scope_id,
                    conversation_id,
                    text,
                    expected_revision,
                })
                .await
            }
            WorkflowAgentCommand::Cancel {
                scope_id,
                conversation_id,
                turn_id,
            } => {
                self.cancel(TurnCancellation {
                    scope_id,
                    conversation_id,
                    turn_id,
                })
                .await
            }
        }
    }

    async fn start_turn(
        &self,
        request: TurnStart,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        let TurnStart {
            scope_id,
            conversation_id,
            text,
            expected_revision,
        } = request;
        let text = text.trim();
        if text.is_empty() {
            return Err(OperationError::InvalidInput(
                "Describe the Workflow change or result you want.".to_owned(),
            ));
        }
        if text.chars().count() > MAX_WORKFLOW_AGENT_INPUT_CHARS {
            return Err(OperationError::InvalidInput(format!(
                "Assistant input is limited to {MAX_WORKFLOW_AGENT_INPUT_CHARS} characters."
            )));
        }
        if !self.inner.services.storage.is_configured() {
            return Err(OperationError::Conflict(
                "Configure or reconnect Spiral before starting an Assistant turn.".to_owned(),
            ));
        }
        if self.inner.services.provider.is_none() {
            return Err(OperationError::External(
                self.inner.readiness.detail().to_owned(),
            ));
        }
        let lease = self
            .inner
            .services
            .catalog_lease
            .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
            .ok_or_else(|| {
                OperationError::Conflict(
                    "Wait for the current recording or catalog change before starting the Assistant."
                        .to_owned(),
                )
            })?;
        let admitted = async {
            let _mutation = self.inner.turn.mutation.lock().await;
            if self.inner.turn.active.recover_lock().is_some() {
                return Err(OperationError::Conflict(
                    "One Assistant turn is already active for this Workflow.".to_owned(),
                ));
            }
            if self.inner.services.workflow.is_active() {
                return Err(OperationError::Conflict(
                    "Wait for the active Workflow run to finish before starting an Assistant turn."
                        .to_owned(),
                ));
            }
            let (mut candidate, conversation_id) = {
                let conversation = self.inner.conversation.recover_lock();
                if scope_id != *self.inner.scope_id.recover_lock()
                    || conversation_id != conversation.conversation_id()
                {
                    return Err(OperationError::Conflict(
                        "The active Assistant conversation changed. Reload it before sending."
                            .to_owned(),
                    ));
                }
                if expected_revision != conversation.revision() {
                    return Err(OperationError::Conflict(
                        "Assistant history changed in another session. Reload it before sending."
                            .to_owned(),
                    ));
                }
                let conversation = &*conversation;
                PublicMessageCapacity::new(conversation.session().messages().len())
                    .ensure_additional(2)
                    .map_err(OperationError::Conflict)?;
                (
                    conversation.clone(),
                    conversation.conversation_id().to_owned(),
                )
            };
            let turn_id = AgentIdentifier::next("agent-turn");
            let user_id = AgentIdentifier::next("agent-user");
            let assistant_id = AgentIdentifier::next("agent-assistant");
            let created_ms = AgentClock::now_millis();
            let conversation = &mut candidate;
            let user_sequence = conversation.session().next_message_sequence();
            conversation.session_mut().messages_mut().push(
                WorkflowAgentMessage::new(user_id, WorkflowAgentRole::User)
                    .with_sequence(user_sequence)
                    .with_phase(WorkflowAgentMessagePhase::Complete)
                    .with_text(text)
                    .with_created_ms(created_ms),
            );
            let assistant_sequence = conversation.session().next_message_sequence();
            conversation.session_mut().messages_mut().push(
                WorkflowAgentMessage::new(assistant_id, WorkflowAgentRole::Assistant)
                    .with_sequence(assistant_sequence)
                    .with_created_ms(created_ms),
            );
            conversation
                .provider_messages_mut()
                .push(ProviderMessage::text("user", text));
            conversation
                .session_mut()
                .set_phase(WorkflowAgentPhase::Responding);
            conversation
                .session_mut()
                .set_active_turn_id(Some(turn_id.clone()));
            conversation.session_mut().set_error(None);
            candidate.advance().map_err(OperationError::External)?;
            candidate.provider_messages_mut().trim_to_provider_bound();
            let cancel = CancellationToken::new();
            let response = self.state_for(&candidate);
            *self.inner.conversation.recover_lock() = candidate;
            *self.inner.turn.active.recover_lock() = Some(ActiveTurn {
                conversation_id: conversation_id.clone(),
                turn_id: turn_id.clone(),
                cancel: cancel.clone(),
                task: None,
            });
            let manager = self.clone();
            let task_conversation_id = conversation_id.clone();
            let task_turn_id = turn_id.clone();
            let turn_task = TurnTask::new(task_conversation_id, task_turn_id).with_runtime(cancel);
            let task = tokio::spawn(async move {
                manager.run_turn(turn_task).await;
            });
            if let Some(current) = self.inner.turn.active.recover_lock().as_mut()
                && current.turn_id == turn_id
                && current.conversation_id == conversation_id
            {
                current.task = Some(task);
            } else {
                task.abort();
            }
            Ok(response)
        }
        .await;
        let released = self.inner.services.catalog_lease.release(lease);
        debug_assert!(released, "agent admission releases its catalog lease once");
        admitted
    }

    async fn cancel(
        &self,
        cancellation: TurnCancellation,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        let TurnCancellation {
            scope_id,
            conversation_id: requested_conversation_id,
            turn_id: requested_turn_id,
        } = cancellation;
        if scope_id != *self.inner.scope_id.recover_lock() {
            return Err(OperationError::Conflict(
                "The active Spiral project changed. Reload Assistant conversations.".to_owned(),
            ));
        }
        let (conversation_id, turn_id) = {
            let mut active = self.inner.turn.active.recover_lock();
            let active = active.as_mut().ok_or_else(|| {
                OperationError::Conflict("No Assistant turn is active.".to_owned())
            })?;
            if active.conversation_id != requested_conversation_id
                || active.turn_id != requested_turn_id
            {
                return Err(OperationError::Conflict(
                    "The active Assistant turn changed. Reload it before stopping.".to_owned(),
                ));
            }
            active.cancel.cancel();
            (active.conversation_id.clone(), active.turn_id.clone())
        };
        {
            let _mutation = self.inner.turn.mutation.lock().await;
            let mut candidate = self.inner.conversation.recover_lock().clone();
            if candidate.ensure_turn(&conversation_id, &turn_id).is_ok() {
                let conversation = &mut candidate;
                conversation
                    .session_mut()
                    .set_phase(WorkflowAgentPhase::Cancelling);
                conversation.session_mut().set_error(None);
                if candidate.advance().is_ok() {
                    *self.inner.conversation.recover_lock() = candidate;
                }
            }
        }
        Ok(self.state())
    }

    pub(crate) async fn shutdown(&self) {
        let active = self.inner.turn.active.recover_lock().take();
        if let Some(mut active) = active {
            active.cancel.cancel();
            if let Some(task) = active.task.take() {
                let _ = task.await;
            }
        }
    }

    async fn run_turn(&self, task: TurnTask) {
        let workflow = self.inner.services.workflow.state().workflow().clone();
        let mut provider_loop = ProviderLoop::new(&task, workflow.revision());
        provider_loop.context.workflow_id = workflow.id().to_owned();
        let identity = TurnIdentity::new(&task.conversation_id, &task.turn_id);
        match self.run_provider_loop(provider_loop).await {
            Ok(()) => self.finish_complete(identity).await,
            Err(error) => {
                self.finish_failed(
                    identity,
                    TurnFailureDetail::new(&error, task.cancel.is_cancelled()),
                )
                .await
            }
        }
    }

    async fn run_provider_loop(&self, provider_loop: ProviderLoop<'_>) -> Result<(), String> {
        let ProviderLoop {
            identity:
                TurnIdentity {
                    conversation_id,
                    turn_id,
                },
            cancel,
            mut context,
            mut budget,
        } = provider_loop;
        let provider = self
            .inner
            .services
            .provider
            .as_ref()
            .ok_or_else(|| ASSISTANT_UNAVAILABLE_DETAIL.to_owned())?;
        let tools =
            WorkflowToolCatalog::for_workflow(self.inner.services.workflow.state().workflow());
        loop {
            budget.admit_provider_iteration()?;
            if cancel.is_cancelled() {
                return Err("Agent turn cancelled.".to_owned());
            }
            let current = self.inner.services.workflow.state();
            let prompt = SYSTEM_PROMPT;
            let mut messages = vec![ProviderMessage::text(
                "system",
                format!(
                    "{prompt}\nCurrent workflow draft (authoritative): {}",
                    serde_json::to_string(current.workflow()).map_err(|error| error.to_string())?
                ),
            )];
            messages.extend(
                self.inner
                    .conversation
                    .recover_lock()
                    .provider_messages()
                    .clone(),
            );
            let delta_manager = self.clone();
            let delta_conversation = conversation_id.to_owned();
            let delta_turn = turn_id.to_owned();
            let response = provider
                .complete(
                    ProviderCompletion::new(&messages, &tools, cancel.clone()),
                    move |delta| {
                        delta_manager.append_text_delta(
                            TurnIdentity::new(&delta_conversation, &delta_turn),
                            delta,
                        );
                    },
                )
                .await
                .map_err(|error| {
                    tracing::warn!(error = %error, "Workflow Assistant provider turn failed");
                    ASSISTANT_UNAVAILABLE_DETAIL.to_owned()
                })?;
            // Admit the whole response before recording its assistant tool
            // calls. A budget rejection must not leave calls with no matching
            // tool-result messages in provider history.
            let provider_message = budget.admit_provider_response(&response)?;
            let identity = TurnIdentity::new(conversation_id, turn_id);
            self.record_provider_response(identity, provider_message)
                .await?;
            if response.tool_calls().is_empty() {
                return Ok(());
            }
            for (index, call) in response.tool_calls().iter().enumerate() {
                if cancel.is_cancelled() {
                    self.record_unexecuted_tool_outputs(
                        identity,
                        UnexecutedTools::new(
                            &response.tool_calls()[index..],
                            "Agent turn cancelled before this tool ran.",
                        ),
                    )
                    .await?;
                    return Err("Agent turn cancelled.".to_owned());
                }
                self.tool_started(identity, call)?;
                let result = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err("Agent turn cancelled.".into()),
                    result = WorkflowToolCatalog::execute(self, call, &mut context) => result,
                };
                let (output, transport_ok) = match result {
                    Ok(output) => (output, true),
                    Err(error) => (json!({"ok": false, "error": error}).to_string(), false),
                };
                let bounded = ToolOutput::bounded_text(output);
                self.complete_tool(
                    identity,
                    ToolRunCompletion::new(call, transport_ok, &bounded),
                )
                .await?;
            }
        }
    }

    async fn update_authored_workflow(
        &self,
        workflow: WorkflowSpec,
        context: &mut ToolContext,
    ) -> Result<String, String> {
        let current = self.inner.services.workflow.state();
        if current.workflow().id() != context.workflow_id {
            return Err("The selected workflow or dataset changed. Use the dataset selector before authoring.".into());
        }
        let (resolved, validation) = self
            .inner
            .services
            .workflow
            .prepare_candidate(workflow)
            .map_err(|error| error.to_string())?;
        if !validation.valid() {
            return Ok(json!({
                "ok": false,
                "validation": validation,
                "resolved_workflow": resolved,
            })
            .to_string());
        }
        let lease = self
            .inner
            .services
            .catalog_lease
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .ok_or_else(|| {
                "A recording or catalog change currently prevents Workflow graph changes."
                    .to_owned()
            })?;
        let saved = self
            .inner
            .services
            .workflow
            .command(WorkflowCommand::apply(
                self.inner.services.workflow.scope_id(),
                context.workflow_revision,
                resolved,
            ))
            .await
            .map_err(|error| error.to_string());
        let released = self.inner.services.catalog_lease.release(lease);
        debug_assert!(released, "agent Workflow save releases its lease once");
        let saved = saved?;
        context.workflow_revision = saved.workflow().revision();
        Ok(json!({
            "ok": true,
            "workflow": saved.workflow(),
            "validation": saved.validation(),
            "resolved_models": saved.workflow().resolved_models_for_agent(
                self.inner.services.storage.table_prefix()
            ),
        })
        .to_string())
    }

    fn append_text_delta(&self, identity: TurnIdentity<'_>, delta: &str) {
        let TurnIdentity {
            conversation_id,
            turn_id,
        } = identity;
        if delta.is_empty() {
            return;
        }
        let mut conversation = self.inner.conversation.recover_lock();
        if conversation.ensure_turn(conversation_id, turn_id).is_err() {
            return;
        }
        if let Some(message) = conversation.session_mut().current_assistant_mut() {
            let mut text = message.text().to_owned();
            text.push_str(delta);
            if text.chars().count() > 32_000 {
                text = text.chars().take(32_000).collect();
            }
            message.set_text(text);
        }
        let _ = conversation.advance();
        drop(conversation);
    }

    async fn record_provider_response(
        &self,
        identity: TurnIdentity<'_>,
        message: ProviderMessage,
    ) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let candidate = self
            .inner
            .conversation
            .recover_lock()
            .with_provider_message(identity, message)?;
        *self.inner.conversation.recover_lock() = candidate;
        Ok(())
    }

    async fn record_unexecuted_tool_outputs(
        &self,
        identity: TurnIdentity<'_>,
        unexecuted: UnexecutedTools<'_>,
    ) -> Result<(), String> {
        let TurnIdentity {
            conversation_id,
            turn_id,
        } = identity;
        let _mutation = self.inner.turn.mutation.lock().await;
        let mut candidate = self.inner.conversation.recover_lock().clone();
        candidate.ensure_turn(conversation_id, turn_id)?;
        candidate
            .provider_messages_mut()
            .append_unexecuted_tool_outputs(unexecuted);
        candidate.provider_messages_mut().trim_to_provider_bound();
        *self.inner.conversation.recover_lock() = candidate;
        Ok(())
    }

    fn tool_started(
        &self,
        identity: TurnIdentity<'_>,
        call: &ProviderToolCall,
    ) -> Result<(), String> {
        let TurnIdentity {
            conversation_id,
            turn_id,
        } = identity;
        let mut conversation = self.inner.conversation.recover_lock();
        conversation.ensure_turn(conversation_id, turn_id)?;
        let message = conversation
            .session_mut()
            .current_assistant_mut()
            .ok_or_else(|| "Assistant message disappeared during tool execution.".to_owned())?;
        message.tools_mut().push(WorkflowAgentToolSummary::new(
            call.id(),
            call.function().name(),
            WorkflowToolCatalog::presentation(call.function().name()).running_summary(),
        ));
        conversation.advance()?;
        drop(conversation);
        Ok(())
    }

    async fn complete_tool(
        &self,
        identity: TurnIdentity<'_>,
        completion: ToolRunCompletion<'_>,
    ) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let candidate = self
            .inner
            .conversation
            .recover_lock()
            .with_completed_tool(identity, completion)?;
        *self.inner.conversation.recover_lock() = candidate;
        Ok(())
    }

    async fn finish_complete(&self, identity: TurnIdentity<'_>) {
        self.finish(TurnFinish::new(identity, WorkflowAgentPhase::Idle))
            .await;
    }

    async fn finish_failed(&self, identity: TurnIdentity<'_>, failure: TurnFailureDetail<'_>) {
        self.finish(
            TurnFinish::new(
                identity,
                if failure.cancelled {
                    WorkflowAgentPhase::Cancelled
                } else {
                    WorkflowAgentPhase::Failed
                },
            )
            .with_error(
                (!failure.cancelled).then(|| ToolPublicText::error(failure.error)),
                failure.cancelled,
            ),
        )
        .await;
    }

    async fn finish(&self, finish: TurnFinish<'_>) {
        let TurnFinish {
            identity:
                TurnIdentity {
                    conversation_id,
                    turn_id,
                },
            phase,
            error,
            cancelled,
        } = finish;
        let _mutation = self.inner.turn.mutation.lock().await;
        if self
            .inner
            .turn
            .active
            .recover_lock()
            .as_ref()
            .is_none_or(|active| {
                active.turn_id != turn_id || active.conversation_id != conversation_id
            })
        {
            return;
        }
        let mut candidate = self.inner.conversation.recover_lock().clone();
        if candidate.ensure_turn(conversation_id, turn_id).is_err() {
            return;
        }
        let conversation = &mut candidate;
        if conversation.session().active_turn_id() != Some(turn_id) {
            return;
        }
        conversation.session_mut().set_phase(phase);
        conversation.session_mut().set_error(error);
        conversation.session_mut().set_active_turn_id(None);
        if let Some(message) = conversation.session_mut().current_assistant_mut() {
            message.set_phase(if cancelled {
                WorkflowAgentMessagePhase::Cancelled
            } else if phase == WorkflowAgentPhase::Failed {
                WorkflowAgentMessagePhase::Failed
            } else {
                WorkflowAgentMessagePhase::Complete
            });
        }
        // A turn may fail after the assistant tool-call batch was
        // recorded but before all tool results were saved. Do not transition
        // that incomplete exchange to an inactive phase: remove the entire
        // tail so the next Retry always starts from provider-valid history.
        conversation
            .provider_messages_mut()
            .remove_incomplete_tool_exchange();
        let _ = candidate.advance();
        *self.inner.conversation.recover_lock() = candidate;
        *self.inner.turn.active.recover_lock() = None;
    }

    fn state_for(&self, conversation: &WorkflowConversation) -> WorkflowAgentSnapshot {
        WorkflowAgentSnapshot::new(self.inner.readiness.clone(), conversation.session().clone())
            .with_unsaved_changes(
                conversation.revision() != *self.inner.turn.saved_revision.recover_lock(),
            )
            .with_history_available(true)
            .with_scope(
                self.inner.scope_id.recover_lock().clone(),
                conversation.revision(),
            )
    }
}

const SYSTEM_PROMPT: &str = r#"You are the Workflow Assistant in Spiral Robotics Studio. Author a single executable Arrow DAG in memory, then let the user choose Run. Only the user's Save button persists the graph and conversation. Use inspect_dataset without arguments to list datasets, then with the actual path to inspect topics and camera schema variants. Also inspect relations and model capabilities as needed. Never invent dataset paths, topics, columns, or functions. Keep the workflow's identity and existing intent when editing it.

A Query with no parents is a SQL source. A Query with parents uses {{node:id}} for each connected input, including joins. A Model has one parent, an image_column, and features with name, description, and concept. It appends nullable Float32 SigLIP scores to all input columns. Result consumes one output. There is no preparation phase or feature-cache relation. Every Run executes inference anew; cross-workflow reuse is a future milestone. No fixed episode/frame key schema is required. Keep useful source identifiers as ordinary columns when the question needs grouping or joins. Schema validation runs before execution. Query max_rows limits the displayed preview, not inference; select the intended cohort and frame sampling in SQL.

For MCAP cameras inspect the actual foxglove.CompressedVideo schema variant. Read packet payloads with spql.mcap.read_topic(dataset, topic, variant); value.data is the encoded payload. Group packets for each member and camera separately. Build a video source with spql.video.from_packets(spql.list.collect(struct_pack(payload := value.data, decode_index := <unique Int64 packet order>)), 'h264', 'annex_b'). Never mix member streams or codecs. Select frames using spql.video.frames(source, spql.video.frame_indices([...])) and decode selected frames with spql.video.decode(frame). Inspect source metadata to determine ordering and sampling; MCAP log_time is not automatically presentation time. Decode after frame selection. Use ordinary SQL to aggregate scores per episode or join other evidence. Null scores are unknown evidence: handle them explicitly rather than treating them as false.

SigLIP scores measure visual text/image matching, not measured brightness or calibrated probability. Explain provisional scoring concepts and thresholds. Inspect only bounded data through run_query. Do not include images, tensors, binary payloads, or credentials in the conversation. Do not execute inference through tools. Save with set_workflow and concisely explain the graph and next Run action."#;

trait WorkflowSpecAgentExt {
    fn resolved_models_for_agent(&self, _namespace: &str) -> Vec<Value>;
}

impl WorkflowSpecAgentExt for WorkflowSpec {
    fn resolved_models_for_agent(&self, _namespace: &str) -> Vec<Value> {
        self.nodes()
            .iter()
            .filter_map(|node| match &node.config() {
                WorkflowNodeConfig::Model {
                    image_column,
                    features,
                } => Some(json!({
                    "node_id": node.id(),
                    "model_id": "siglip-base",
                    "image_column": image_column,
                    "features": features,
                })),
                _ => None,
            })
            .collect()
    }
}

trait SpiralQuerySnapshotAgentExt {
    fn serialize_for_agent_prompt(self) -> String;
}

impl SpiralQuerySnapshotAgentExt for SpiralQuerySnapshot {
    fn serialize_for_agent_prompt(self) -> String {
        let row_count = self.rows().len();
        let source_truncated = self.truncated();
        let elapsed_ms = self.elapsed_ms();
        let semantic = self
            .columns()
            .iter()
            .map(|column| {
                (
                    column.name().to_owned(),
                    column.semantic_type().map(str::to_owned),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut rows = Vec::new();
        let mut used = 0_usize;
        for row in self.rows().iter().take(MAX_WORKFLOW_AGENT_QUERY_ROWS) {
            let mut safe = serde_json::Map::new();
            for (name, value) in row {
                let converted = value.clone().safe_for_agent_prompt(
                    semantic
                        .get(name.as_str())
                        .and_then(|value| value.as_deref()),
                );
                let encoded = converted.to_string();
                if used.saturating_add(encoded.len()) > MAX_QUERY_PROMPT_BYTES {
                    safe.insert(name.clone(), json!({"type":"truncated"}));
                    break;
                }
                used = used.saturating_add(encoded.len());
                safe.insert(name.clone(), converted);
            }
            rows.push(Value::Object(safe));
            if used >= MAX_QUERY_PROMPT_BYTES {
                break;
            }
        }
        let mut columns = self.columns().to_vec();
        let mut truncated = source_truncated || used >= MAX_QUERY_PROMPT_BYTES;
        loop {
            let encoded = json!({
                "columns": &columns,
                "rows": &rows,
                "row_count": row_count,
                "truncated": truncated,
                "elapsed_ms": elapsed_ms,
            })
            .to_string();
            if encoded.len() <= MAX_QUERY_PROMPT_BYTES {
                return encoded;
            }
            truncated = true;
            if rows.pop().is_some() {
                continue;
            }
            if columns.pop().is_some() {
                continue;
            }
            return json!({
                "row_count": row_count,
                "truncated": true,
                "error": "Query metadata exceeded the bounded prompt payload limit."
            })
            .to_string();
        }
    }
}

trait AgentPromptCell {
    fn safe_for_agent_prompt(self, semantic_type: Option<&str>) -> Value;
}

impl AgentPromptCell for Value {
    fn safe_for_agent_prompt(self, semantic_type: Option<&str>) -> Value {
        if let Some(semantic_type) = semantic_type {
            return json!({"type":"media_or_tensor","semantic_type":semantic_type});
        }
        match self {
            Value::String(value) if value.starts_with("data:") => json!({"type":"media"}),
            Value::String(value) => Value::String(value.chars().take(MAX_CELL_CHARS).collect()),
            Value::Array(value) if value.len() > 32 => json!({"type":"array","length":value.len()}),
            Value::Object(value) if value.len() > 32 => {
                json!({"type":"object","fields":value.len()})
            }
            value => value,
        }
    }
}

struct ToolPublicText;

impl ToolPublicText {
    fn label(value: &str) -> String {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect()
    }

    fn error(error: &str) -> String {
        error
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(800)
            .collect()
    }

    fn detail(detail: &str) -> Option<String> {
        let value = serde_json::from_str::<Value>(detail).unwrap_or(Value::Null);
        if let Some(error) = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.trim().is_empty())
        {
            return Some(Self::error(error));
        }
        if let Some(issue) = value
            .get("validation")
            .and_then(|validation| validation.get("errors"))
            .and_then(Value::as_array)
            .and_then(|errors| errors.first())
        {
            let message = issue
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The graph is not valid.");
            let recovery = issue
                .get("recovery")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Some(Self::error(&format!("{message} {recovery}")));
        }
        match value.get("phase").and_then(Value::as_str) {
            Some("cancelled" | "interrupted") => None,
            Some("failed") => Some("Workflow run failed without a public error detail.".to_owned()),
            _ if value.is_null() => Some(Self::error(detail)),
            _ => Some("The tool failed without a public error detail.".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_and_long_cells_never_enter_prompt_payloads() {
        assert_eq!(
            Value::String("data:image/png;base64,secret".to_owned()).safe_for_agent_prompt(None),
            json!({"type":"media"})
        );
        assert_eq!(
            json!([1, 2, 3]).safe_for_agent_prompt(Some("spql.image.Image")),
            json!({"type":"media_or_tensor","semantic_type":"spql.image.Image"})
        );
        let long = "x".repeat(MAX_CELL_CHARS + 10);
        assert_eq!(
            Value::String(long)
                .safe_for_agent_prompt(None)
                .as_str()
                .unwrap()
                .len(),
            MAX_CELL_CHARS
        );
    }

    #[test]
    fn serialized_query_prompt_is_hard_bounded_including_schema_and_json_overhead() {
        let columns = (0..40)
            .map(|index| {
                spiral_robotics_common::relations::QueryColumn::new(
                    format!("field_{index}_{}", "x".repeat(900)),
                    format!("Struct<{}>", "y".repeat(900)),
                    true,
                )
            })
            .collect::<Vec<_>>();
        let row = columns
            .iter()
            .map(|column| (column.name().to_owned(), Value::String("value".repeat(100))))
            .collect();
        let encoded = SpiralQuerySnapshot::new(columns, vec![row], false)
            .with_elapsed_ms(1)
            .serialize_for_agent_prompt();
        assert!(encoded.len() <= MAX_QUERY_PROMPT_BYTES);
        let payload = serde_json::from_str::<Value>(&encoded).unwrap();
        assert_eq!(payload["row_count"], 1);
        assert_eq!(payload["truncated"], true);
    }

    #[tokio::test]
    async fn history_and_stop_do_not_wait_for_storage_or_tool_cleanup() {
        let store = Arc::new(spiral_robotics_storage::ProjectStore::workflow_test());
        let storage = Arc::new(StorageManager::new(store.clone()));
        let workflow = Arc::new(WorkflowManager::new(storage.clone()));
        let agent = WorkflowAgentManager::new(
            storage,
            workflow.clone(),
            Arc::new(CatalogMutationLease::default()),
        );
        let mut conversation = WorkflowConversation::new(workflow.state().workflow().id());
        let conversation_id = conversation.conversation_id().to_owned();
        let turn_id = "stop-test".to_owned();
        conversation
            .session_mut()
            .set_active_turn_id(Some(turn_id.clone()));
        conversation
            .session_mut()
            .set_phase(WorkflowAgentPhase::Responding);
        agent.select_conversation(conversation);
        let cancel = CancellationToken::new();
        let (release, pending) = tokio::sync::oneshot::channel::<()>();
        *agent.inner.turn.active.recover_lock() = Some(ActiveTurn {
            conversation_id: conversation_id.clone(),
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
            task: Some(tokio::spawn(async move {
                let _ = pending.await;
            })),
        });
        let _exclusive_storage = store.connect_exclusive().await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            agent.record_provider_response(
                TurnIdentity::new(&conversation_id, &turn_id),
                ProviderMessage::text("assistant", "Draft response"),
            ),
        )
        .await
        .expect("history changes must not write storage")
        .unwrap();
        let state = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            agent.cancel(TurnCancellation {
                scope_id: agent.state().scope_id().into(),
                conversation_id,
                turn_id,
            }),
        )
        .await
        .expect("Stop must acknowledge before tool cleanup")
        .unwrap();
        assert_eq!(state.session().phase(), WorkflowAgentPhase::Cancelling);
        assert!(state.unsaved_changes());
        assert!(cancel.is_cancelled());
        assert!(
            agent
                .inner
                .turn
                .active
                .recover_lock()
                .as_ref()
                .unwrap()
                .task
                .is_some()
        );
        release.send(()).unwrap();
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn authoring_rejects_old_run_tools_at_the_backend_boundary() {
        let settings = spiral_robotics_storage::SpiralConnectionSettings::new(
            "http://127.0.0.1:1".to_owned(),
            "http://127.0.0.1:2".to_owned(),
        )
        .unwrap();
        let database = Arc::new(spiral_robotics_storage::AdbcDatabase::new(settings.clone()));
        let project = spiral_robotics_storage::ProjectStore::new(
            settings,
            "authoring_rejection".to_owned(),
            database,
        )
        .unwrap();
        let storage = Arc::new(StorageManager::new(Arc::new(project)));
        let workflow = Arc::new(WorkflowManager::new(storage.clone()));
        let agent = WorkflowAgentManager::new(
            storage,
            workflow.clone(),
            Arc::new(CatalogMutationLease::default()),
        );
        let before = workflow.state();
        let mut context = ToolContext {
            workflow_id: before.workflow().id().to_owned(),
            workflow_revision: before.workflow().revision(),
        };
        for name in ["run_workflow", "wait_for_run"] {
            let call = ProviderToolCall::new("unadvertised-call", name, "{}");
            let error = WorkflowToolCatalog::execute(&agent, &call, &mut context)
                .await
                .unwrap_err();
            assert!(error.contains("only authors"));
            assert_eq!(workflow.state(), before);
            assert!(!workflow.is_active());
        }
    }

    #[test]
    fn tool_schema_exposes_only_authoring_and_inspection() {
        let definitions =
            WorkflowToolCatalog::for_workflow(&WorkflowSpec::new("schema-test", "Schema test"));
        let names = definitions
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "inspect_dataset",
                "list_relations",
                "describe_relations",
                "list_model_capabilities",
                "set_workflow",
                "run_query",
            ]
        );
        assert_eq!(
            definitions[5]["function"]["parameters"]["properties"]["max_rows"]["maximum"],
            MAX_WORKFLOW_AGENT_QUERY_ROWS
        );
        let workflow = &definitions[4]["function"]["parameters"]["properties"]["workflow"];
        assert!(workflow["properties"].get("id").is_none());
        assert!(workflow["properties"].get("revision").is_none());
        assert_eq!(workflow["required"], json!(["name", "nodes", "edges"]));
        let variants = workflow["properties"]["nodes"]["items"]["oneOf"]
            .as_array()
            .expect("set_workflow nodes have discriminated variants");
        assert_eq!(variants.len(), 4);
        assert_eq!(variants[0]["properties"]["kind"]["const"], "table");
        assert_eq!(variants[1]["properties"]["kind"]["const"], "model");
        assert_eq!(variants[1]["properties"]["image_column"]["type"], "string");
        assert_eq!(variants[2]["properties"]["kind"]["const"], "query");
        assert!(variants[1]["properties"].get("sample_every_ms").is_none());
        assert!(variants[1]["properties"].get("media_column").is_none());
        assert!(variants[1]["properties"].get("concepts").is_none());
        let feature = &variants[1]["properties"]["features"]["items"];
        assert_eq!(
            feature["properties"]["name"]["pattern"],
            "^[a-z_][a-z0-9_]*$"
        );
        assert!(feature["properties"].get("applicability").is_none());
    }

    #[test]
    fn public_tool_receipts_are_specific_bounded_and_secret_free() {
        let saved = ToolCompletion::for_tool(
            "set_workflow",
            true,
            &json!({
                "ok": true,
                "workflow": {"name": "Holding evidence", "revision": 7},
                "provider_secret": "must-not-leak"
            })
            .to_string(),
        );
        assert_eq!(saved.summary, "Updated Holding evidence · r7");
        let receipt = saved.receipt.expect("saved graph receipt");
        assert_eq!(receipt.revision(), 7);
        assert!(!serde_json::to_string(&receipt).unwrap().contains("secret"));

        assert_eq!(
            ToolCompletion::for_tool(
                "list_relations",
                true,
                &json!({"relations": ["a.b", "c.d"]}).to_string()
            )
            .summary,
            "Found 2 Spiral relations"
        );
        assert_eq!(
            ToolCompletion::for_tool(
                "run_query",
                true,
                &json!({"row_count": 12, "rows": [1], "truncated": true}).to_string()
            )
            .summary,
            "Inspected 12 rows · bounded preview"
        );
    }

    #[test]
    fn rejected_or_oversized_tool_output_is_publicly_failed() {
        let rejected = ToolCompletion::for_tool(
            "set_workflow",
            false,
            &json!({
                "ok": false,
                "validation": {"errors": [{
                    "message": "Query is missing a placeholder.",
                    "recovery": "Reference the connected Table."
                }]}
            })
            .to_string(),
        );
        assert_eq!(rejected.summary, "Couldn’t update the workflow");
        assert!(rejected.receipt.is_none());
        assert_eq!(
            ToolPublicText::detail(
                &json!({
                    "validation": {"errors": [{
                        "message": "Query is missing a placeholder.",
                        "recovery": "Reference the connected Table."
                    }]}
                })
                .to_string()
            ),
            Some("Query is missing a placeholder. Reference the connected Table.".to_owned())
        );

        assert_eq!(
            ToolPublicText::detail(
                &json!({"ok": false, "phase": "cancelled", "error": null}).to_string()
            ),
            None
        );
        assert_eq!(
            ToolPublicText::detail(
                &json!({"ok": false, "phase": "failed", "error": null}).to_string()
            ),
            Some("Workflow run failed without a public error detail.".to_owned())
        );

        let bounded = ToolOutput::bounded_text("x".repeat(MAX_TOOL_OUTPUT_BYTES + 1));
        assert_eq!(
            serde_json::from_str::<Value>(&bounded).unwrap()["ok"],
            false
        );
        assert!(
            ToolCompletion::for_tool("run_query", false, &bounded)
                .receipt
                .is_none()
        );
    }

    #[test]
    fn turn_budget_bounds_provider_iterations_and_tool_calls() {
        let mut budget = TurnBudget::default();
        for _ in 0..MAX_TOOL_ITERATIONS {
            budget.admit_provider_iteration().unwrap();
        }
        assert!(budget.admit_provider_iteration().is_err());

        let mut budget = TurnBudget::default();
        budget.admit_tool_calls(MAX_TOOL_CALLS / 2).unwrap();
        budget.admit_tool_calls(MAX_TOOL_CALLS / 2).unwrap();
        assert!(budget.admit_tool_calls(1).is_err());
    }

    #[test]
    fn budget_overflow_rejects_response_before_tool_calls_are_retained() {
        let mut budget = TurnBudget {
            provider_iterations: 0,
            tool_calls: MAX_TOOL_CALLS,
        };
        let response = ProviderResponse::test_response("", vec![tool_call("overflow")]);
        assert!(budget.admit_provider_response(&response).is_err());
    }

    #[test]
    fn public_state_has_no_provider_secret_field() {
        let state = WorkflowAgentSnapshot::default();
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("LLM_API_KEY"));
    }

    #[test]
    fn public_provider_readiness_is_operator_facing_and_provider_agnostic() {
        let ready = ProviderReadiness::public(true);
        assert!(ready.configured());
        assert!(matches!(ready, WorkflowAgentReadiness::Ready { .. }));
        assert_eq!(ready.detail(), ASSISTANT_READY_DETAIL);

        let unavailable = ProviderReadiness::public(false);
        assert!(!unavailable.configured());
        assert!(matches!(unavailable, WorkflowAgentReadiness::Error { .. }));
        assert_eq!(unavailable.detail(), ASSISTANT_UNAVAILABLE_DETAIL);

        for readiness in [ready, unavailable] {
            let encoded = serde_json::to_string(&readiness).unwrap();
            for forbidden in [
                "LLM_",
                "env var",
                "environment variable",
                "openrouter.ai",
                "openai/gpt-5.6-terra",
            ] {
                assert!(!encoded.contains(forbidden), "leaked {forbidden}");
            }
        }
    }

    #[test]
    fn new_turn_checks_the_public_message_bound_before_mutation() {
        assert!(
            PublicMessageCapacity::new(MAX_PUBLIC_MESSAGES - 2)
                .ensure_additional(2)
                .is_ok()
        );
        assert!(
            PublicMessageCapacity::new(MAX_PUBLIC_MESSAGES - 1)
                .ensure_additional(2)
                .is_err()
        );
        assert!(
            PublicMessageCapacity::new(MAX_PUBLIC_MESSAGES)
                .ensure_additional(1)
                .is_err()
        );
    }

    #[test]
    fn provider_response_candidate_does_not_mutate_live_history_before_persistence() {
        let mut conversation = WorkflowConversation::new("default");
        let conversation_id = conversation.conversation_id().to_owned();
        conversation
            .session_mut()
            .set_phase(WorkflowAgentPhase::Responding);
        conversation
            .session_mut()
            .set_active_turn_id(Some("turn-1".to_owned()));
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::text("user", "inspect relations"));

        let candidate = conversation
            .with_provider_message(
                TurnIdentity::new(&conversation_id, "turn-1"),
                ProviderMessage::text("assistant", "I found them."),
            )
            .unwrap();

        assert_eq!(
            conversation.provider_messages().len(),
            1,
            "a failed candidate persistence must leave the live history unchanged"
        );
        assert_eq!(candidate.provider_messages().len(), 2);
    }

    #[test]
    fn tool_completion_commits_public_receipt_and_provider_result_in_one_candidate() {
        for phase in [
            WorkflowAgentPhase::Responding,
            WorkflowAgentPhase::Cancelling,
        ] {
            let mut conversation = WorkflowConversation::new("default");
            let conversation_id = conversation.conversation_id().to_owned();
            let call = ProviderToolCall::new("call-1", "set_workflow", "{}");
            conversation.session_mut().set_phase(phase);
            conversation
                .session_mut()
                .set_active_turn_id(Some("turn-1".to_owned()));
            conversation.session_mut().messages_mut().push(
                WorkflowAgentMessage::new("assistant-1", WorkflowAgentRole::Assistant)
                    .with_phase(WorkflowAgentMessagePhase::Streaming)
                    .with_tools(vec![WorkflowAgentToolSummary::new(
                        "call-1",
                        "set_workflow",
                        "Saving",
                    )]),
            );
            conversation
                .provider_messages_mut()
                .push(ProviderMessage::assistant(None, vec![call.clone()]));
            let detail =
                json!({"ok": true, "workflow": {"name": "Saved graph", "revision": 3}}).to_string();
            let candidate = conversation
                .with_completed_tool(
                    TurnIdentity::new(&conversation_id, "turn-1"),
                    ToolRunCompletion::new(&call, true, &detail),
                )
                .unwrap();
            let original = &conversation;
            assert_eq!(original.provider_messages().len(), 1);
            assert_eq!(
                original.session().messages()[0].tools()[0].phase(),
                WorkflowAgentToolPhase::Running
            );
            let completed = &candidate;
            assert_eq!(completed.provider_messages().len(), 2);
            assert_eq!(
                completed.provider_messages()[1].tool_call_id(),
                Some("call-1")
            );
            assert_eq!(
                completed.session().messages()[0].tools()[0].phase(),
                WorkflowAgentToolPhase::Complete
            );
            assert_eq!(
                completed.session().messages()[0].tools()[0]
                    .receipt()
                    .unwrap()
                    .revision(),
                3
            );
            assert_eq!(completed.session().phase(), phase);
            assert!(candidate.revision() > conversation.revision());
            assert!(candidate.persisted().is_ok());
            assert!(
                conversation
                    .with_completed_tool(
                        TurnIdentity::new(&conversation_id, "stale-turn"),
                        ToolRunCompletion::new(&call, true, &detail)
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn mid_batch_cancel_records_results_for_every_unexecuted_call() {
        let calls = vec![tool_call("first"), tool_call("second")];
        let mut messages = vec![ProviderMessage::assistant(None, calls.clone())];
        messages.append_unexecuted_tool_outputs(UnexecutedTools::new(&calls, "cancelled"));
        let result_ids = messages
            .iter()
            .filter_map(ProviderMessage::tool_call_id)
            .collect::<Vec<_>>();
        assert_eq!(result_ids, ["first", "second"]);
        assert!(messages[1].content().unwrap().contains("cancelled"));
    }

    fn tool_call(id: &str) -> ProviderToolCall {
        ProviderToolCall::new(id, "list_relations", "{}")
    }
}
