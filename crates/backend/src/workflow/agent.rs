//! Backend-owned Workflow agent, tool loop, durable session, and query state.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

use crate::OperationError;
use crate::data::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};
use crate::sync::MutexRecovery;
use serde_json::{Value, json};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_common::workflow::{
    WorkflowAgentArtifactReceipt, WorkflowAgentCommand, WorkflowAgentMessage,
    WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentReadiness, WorkflowAgentRole,
    WorkflowAgentSession, WorkflowAgentSnapshot, WorkflowAgentToolPhase, WorkflowAgentToolSummary,
    WorkflowCommand, WorkflowNodeConfig, WorkflowSpec,
};

use crate::workflow::manager::WorkflowManager;
use crate::workflow::provider::{
    OpenAiProvider, ProviderCompletion, ProviderConfig, ProviderResponse,
};
use spiral_robotics_storage::workflow_assistant::{
    ProviderMessage, ProviderMessageHistory, ProviderToolCall, WorkflowConversation,
};
use spiral_robotics_storage::workflows::MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION;
use spiral_robotics_storage::{StorageManager, StorageSchema};

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
    recovery_pending: AtomicBool,
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
                    recovery_pending: AtomicBool::new(false),
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
                self.persist_recovery().await?;
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
        let interrupted = conversation.interrupt()?;
        *self.inner.conversation.recover_lock() = conversation;
        *self.inner.scope_id.recover_lock() = AgentIdentifier::next("agent-scope");
        self.inner.turn.recovery_pending.store(
            interrupted || stored.is_none() || stored.is_some_and(|stored| stored.version() == 2),
            Ordering::SeqCst,
        );
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
        *self.inner.conversation.recover_lock() = conversation;
        *self.inner.scope_id.recover_lock() = AgentIdentifier::next("agent-scope");
    }

    pub(crate) async fn persist_recovery(&self) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        if self.inner.turn.recovery_pending.load(Ordering::SeqCst) {
            let conversation = self.inner.conversation.recover_lock().clone();
            self.persist_conversation(&conversation).await?;
            self.inner
                .turn
                .recovery_pending
                .store(false, Ordering::SeqCst);
        }
        Ok(())
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
            self.persist_conversation(&candidate)
                .await
                .map_err(OperationError::External)?;
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
        let (conversation_id, turn_id, task) = {
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
            (
                active.conversation_id.clone(),
                active.turn_id.clone(),
                active.task.take(),
            )
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
                if candidate.advance().is_ok()
                    && self.persist_conversation(&candidate).await.is_ok()
                {
                    *self.inner.conversation.recover_lock() = candidate;
                }
            }
        }
        if let Some(task) = task {
            let _ = task.await;
        }
        self.finish_failed(
            TurnIdentity::new(&conversation_id, &turn_id),
            TurnFailureDetail::new("Agent turn cancelled.", true),
        )
        .await;
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
        let tools = WorkflowToolCatalog::definitions();
        loop {
            budget.admit_provider_iteration()?;
            if cancel.is_cancelled() {
                return Err("Agent turn cancelled.".to_owned());
            }
            let mut messages = vec![ProviderMessage::text("system", SYSTEM_PROMPT)];
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
            // Admit the whole response before persisting its assistant tool
            // calls. A budget rejection must not leave calls with no matching
            // tool-result messages in durable provider history.
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
                let result = self.execute_tool(call, &mut context).await;
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

    async fn execute_tool(
        &self,
        call: &ProviderToolCall,
        context: &mut ToolContext,
    ) -> Result<String, String> {
        let args: Value = serde_json::from_str(call.function().arguments())
            .map_err(|error| format!("Tool arguments are malformed: {error}"))?;
        match call.function().name() {
            "list_relations" => {
                let response = self.inner.services.storage.relation_catalog().await?;
                Ok(json!({
                    "relations": response.relations().iter().map(|relation| relation.name()).collect::<Vec<_>>(),
                    "complete": response.complete(),
                    "limitation": response.limitation(),
                }).to_string())
            }
            "describe_relations" => {
                let names = args
                    .get("relations")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "describe_relations requires a relations array.".to_owned())?;
                if names.len() > 20 {
                    return Err("Describe at most 20 relations per tool call.".to_owned());
                }
                let discovered = self.inner.services.storage.relation_catalog().await?;
                let requested = names.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                let relations = discovered
                    .relations()
                    .iter()
                    .filter(|relation| requested.contains(&relation.name()))
                    .collect::<Vec<_>>();
                Ok(serde_json::to_string(&json!({"relations": relations}))
                    .map_err(|error| error.to_string())?)
            }
            "list_model_capabilities" => Ok(json!({
                "models": [{
                    "id": "siglip-base",
                    "label": "SigLIP base",
                    "max_features": 32,
                    "device_policy": "Metal when available; CPU fallback"
                }]
            })
            .to_string()),
            "set_workflow" => {
                let mut workflow_value = args
                    .get("workflow")
                    .cloned()
                    .ok_or_else(|| "set_workflow requires workflow.".to_owned())?;
                let workflow_object = workflow_value
                    .as_object_mut()
                    .ok_or_else(|| "set_workflow requires a workflow object.".to_owned())?;
                // Identity and revision are server-owned. Keeping them out of the
                // provider schema prevents a harmless model guess from becoming a
                // visible failed tool step in an otherwise successful turn.
                workflow_object.insert(
                    "id".to_owned(),
                    Value::String(context.workflow_id.clone()),
                );
                workflow_object.insert("revision".to_owned(), json!(context.workflow_revision));
                let workflow: WorkflowSpec = serde_json::from_value(workflow_value)
                    .map_err(|error| format!("Workflow shape is invalid: {error}"))?;
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
            "run_workflow" | "wait_for_run" => Err("The Assistant only authors workflows. Run computation explicitly from the workflow controls.".to_owned()),
            "run_query" => {
                let sql = args
                    .get("sql")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "run_query requires SQL.".to_owned())?;
                let max_rows = args.get("max_rows").and_then(Value::as_u64).unwrap_or(20) as usize;
                if max_rows == 0 || max_rows > MAX_WORKFLOW_AGENT_QUERY_ROWS {
                    return Err(format!(
                        "run_query max_rows must be between 1 and {MAX_WORKFLOW_AGENT_QUERY_ROWS}."
                    ));
                }
                let response = self.inner.services.storage.query(sql, max_rows).await?;
                Ok(response.serialize_for_agent_prompt())
            }
            other => Err(format!("Unknown Workflow tool {other:?}.")),
        }
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
        self.persist_conversation(&candidate).await?;
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
        self.persist_conversation(&candidate).await?;
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
            ToolPresentation::for_name(call.function().name()).running_summary(),
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
        self.persist_conversation(&candidate).await?;
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
        // A turn may fail after the assistant tool-call batch was durably
        // recorded but before all tool results were saved. Do not transition
        // that incomplete exchange to an inactive phase: remove the entire
        // tail so the next Retry always starts from provider-valid history.
        conversation
            .provider_messages_mut()
            .remove_incomplete_tool_exchange();
        let _ = candidate.advance();
        if let Err(persist_error) = self.persist_conversation(&candidate).await {
            let conversation = &mut candidate;
            conversation
                .session_mut()
                .set_phase(WorkflowAgentPhase::Failed);
            conversation.session_mut().set_error(Some(format!(
                "Could not persist Assistant history: {}",
                ToolPublicText::error(&persist_error)
            )));
        }
        // Even when durable storage is temporarily unavailable, install only
        // the structurally valid candidate. A later Retry can persist it; the
        // live conversation is never left with an inactive orphaned tool exchange.
        *self.inner.conversation.recover_lock() = candidate;
        *self.inner.turn.active.recover_lock() = None;
    }

    fn state_for(&self, conversation: &WorkflowConversation) -> WorkflowAgentSnapshot {
        WorkflowAgentSnapshot::new(self.inner.readiness.clone(), conversation.session().clone())
            .with_history_available(true)
            .with_scope(
                self.inner.scope_id.recover_lock().clone(),
                conversation.revision(),
            )
    }

    async fn persist_conversation(
        &self,
        conversation: &WorkflowConversation,
    ) -> Result<(), String> {
        self.inner
            .services
            .storage
            .save_workflow_conversation(conversation.workflow_id(), &conversation.persisted()?)
            .await
    }
}

const SYSTEM_PROMPT: &str = r#"You are the Workflow Assistant in Spiral Robotics Studio. Work only with relations already materialized in the active Spiral project. Never import Hugging Face datasets or MCAP files. Build relation-first DAGs using Table, optional siglip-base Model, Query, and exactly one terminal Result. Name workflows and nodes for the user's intent. Query SQL is read-only. Model output relations and feature columns are resolved by the server. Use tools to inspect authoritative schemas, validate and save the graph, and inspect bounded read-only query results. You only author workflows; never start or wait for computation. The user explicitly runs computation from the workflow controls.

SigLIP enrichment operates on a SQL-defined candidate cohort. The graph shape is source Table → candidate Query → Model → final Query → Result. Also connect the candidate Query directly to the final Query so it can join candidate identity to the Model feature cache. Other evidence such as a joint-samples Table joins only at the final Query. Reference every connected upstream node through its exact placeholder, for example {{node:candidates}}, {{node:visual_features}}, and {{node:joint_samples}}. Every ordinary durable relation used by SQL must be represented by a connected Table node; the only exception is the actual managed media relation needed to resolve a manifest's media value. Never hide a joint-state or other scalar relation inside candidate SQL: add it as a Table root, join it in the final Query, apply its predicates there, and return the matching scalar evidence in Results. A Model must trace to exactly one authoritative source Table; the server derives one cross-workflow feature-cache relation from that source identity.

The candidate Query must return non-null episode_id Utf8, stream_id Utf8, segment_index UInt32, frame_index UInt64, episode_time_ns UInt64, and every decoded spql.image.Image media_field named by its feature definitions. Author periodic or downsampled cohorts in this read-only SQL, not as Model configuration and not through a mutation tool. An inspected durable video manifest may expose episode_id, stream_id, segment_index, timeline_offset_ns, fps, frames, and media_relation. Use the actual managed media relation discovered with tools. Spiral frame selection happens before decode. Join the manifest row to that managed media relation, then use the table-function form `CROSS JOIN LATERAL spql.video.frames(asset.media, spql.video.frame_slice(CAST(0 AS BIGINT), CAST(manifest.frames AS BIGINT), CAST(<positive integral stride derived from manifest.fps and the requested cadence> AS BIGINT))) AS selected_frame`. Read `selected_frame.request_index` and `selected_frame.frame`; decode the latter with `spql.video.decode`. request_index is the zero-based selected output ordinal, not the source video frame number. For a zero-start fixed stride, emit the source identity as `CAST(selected_frame.request_index AS UINT64) * CAST(<the same stride> AS UINT64) AS frame_index`, and use that source frame index in the episode_time_ns calculation. request_index is non-null, so never wrap it in COALESCE. BIGINT casts are for frame_slice arguments only: keep segment_index as UInt32 and derive frame_index and episode_time_ns with UINT64 operands/casts so the five returned identity fields retain their required unsigned types. The pinned backend processor owns bicubic SigLIP resizing.

Spiral does not support LEAD or LAG window functions, and it supports LATERAL only for table functions such as `spql.video.frames`, not for derived SELECT subqueries. To find the sampled frame immediately before a visual transition, use ordinary self-joins over the candidate and feature placeholders: join the next candidate on the same episode_id, stream_id, and segment_index with `next_candidate.frame_index = current_candidate.frame_index + CAST(<the same fixed sampling stride used by frame_slice> AS UINT64)`, then join the feature cache once for the current key and once for the next key. Unless the user specifies another calibrated boundary, define the transition as `current_features.<feature> < 0.5 AND next_features.<feature> >= 0.5`; a mere score increase is not a release transition and can make evidence recovery unbounded. For nearby joint-state evidence, first inspect its actual timestamp cadence, then use a narrow bounded temporal join such as `joint_state.episode_time_ns + CAST(<observed tolerance ns> AS UINT64) >= current_candidate.episode_time_ns AND joint_state.episode_time_ns <= current_candidate.episode_time_ns + CAST(<observed tolerance ns> AS UINT64)`. Aggregate the scalar evidence by the stable candidate identity (for example MAX(follow_error_degrees)) before joining decoded images, so the engine never ranks or carries image values across an expanded all-prior-rows join. Decode a selected table-function frame directly in the SELECT list rather than wrapping decode in a lateral derived SELECT. Do not emit LEAD, LAG, or a lateral derived subquery.

Use at most one `spql.video.frames` generator in each SELECT scope because generator output names such as request_index collide even when aliases differ. For paired-camera evidence, carry the candidate Query's already-decoded primary image into the final Query and use one frames generator only for the second camera. If both images must be decoded independently, isolate each generator in its own CTE and join the two scalar-keyed CTEs afterward.

Write SQL identifiers conservatively. Never use SQL keywords such as top, table, result, query, select, group, order, or window as unquoted relation, CTE, or table aliases; use descriptive aliases such as top_cam, gripper_cam, candidate_frames, and joint_state instead. Before saving a graph, inspect the complete generated SQL for reserved-word aliases and replace them.

Define every requested Model feature explicitly with a safe descriptive name ending in _feature, a short human description, the SigLIP text concept, and applicability containing the source relation, candidate media field, and optional stream_ids. Inspect the Studio namespace's feature_registry relation with the ordinary relation/query tools before introducing a name: a compatible source-scoped definition is reused across workflows, while a conflicting same-name definition is rejected. The Model cache contains only the five stable identity fields plus nullable feature columns such as is_raining_feature or is_gripper_open_feature; it never contains decoded images. The backend reuses matching cells across workflows over the same source and fills only missing applicable feature cells. A final Query joins the candidate cohort, feature cache, and optional joint state, then may recover full-resolution image evidence from an actual managed media relation using only read-only Spiral functions and fields verified through the relation tools.

Never invent relations, columns, managed media values, model support, run state, or SPQL functions. Do not request or reveal provider credentials. Do not put binary, tensor, image, or full media payloads in the conversation. Keep final answers concise and structured: state what graph was saved, summarize bounded evidence, and point the user to the authoritative graph or Results."#;

struct WorkflowToolCatalog;

impl WorkflowToolCatalog {
    fn definitions() -> Vec<Value> {
        let feature_definition_schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "maxLength": 96,
                    "pattern": "^[a-z_][a-z0-9_]*_feature$"
                },
                "description": {"type": "string", "maxLength": 240},
                "concept": {"type": "string", "maxLength": 80},
                "applicability": {
                    "type": "object",
                    "properties": {
                        "source_relation": {"type": "string"},
                        "media_field": {"type": "string"},
                        "stream_ids": {
                            "type": "array",
                            "maxItems": 8,
                            "items": {"type": "string", "maxLength": 128}
                        }
                    },
                    "required": ["source_relation", "media_field"],
                    "additionalProperties": false
                }
            },
            "required": ["name", "description", "concept", "applicability"],
            "additionalProperties": false
        });
        vec![
            Self::function(
                "list_relations",
                "List qualified Spiral relations in the active project.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            Self::function(
                "describe_relations",
                "Describe fields and semantic types for qualified Spiral relations.",
                json!({"type":"object","properties":{"relations":{"type":"array","items":{"type":"string"},"maxItems":20}},"required":["relations"],"additionalProperties":false}),
            ),
            Self::function(
                "list_model_capabilities",
                "List executable Workflow Model capabilities and input contracts.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            Self::function(
                "set_workflow",
                "Validate and save a complete relation-first Workflow graph. Provide its name, nodes, and edges; the server supplies identity and revision. A Model enriches one upstream candidate Query, and the final Query may join that cohort, the source-scoped feature cache, and other Table roots when every connected placeholder is referenced. Server-owned Model outputs are resolved automatically.",
                json!({
                    "type": "object",
                    "properties": {
                        "workflow": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "nodes": {
                                    "type": "array",
                                    "minItems": 3,
                                    "items": {
                                        "oneOf": [
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "id": {"type": "string"},
                                                    "name": {"type": "string"},
                                                    "kind": {"const": "table"},
                                                    "relation": {"type": "string"}
                                                },
                                                "required": ["id", "name", "kind", "relation"],
                                                "additionalProperties": false
                                            },
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "id": {"type": "string"},
                                                    "name": {"type": "string"},
                                                    "kind": {"const": "model"},
                                                    "model_id": {"const": "siglip-base"},
                                                    "features": {
                                                        "type": "array",
                                                        "minItems": 1,
                                                        "maxItems": 32,
                                                        "items": feature_definition_schema
                                                    }
                                                },
                                                "required": ["id", "name", "kind", "model_id", "features"],
                                                "additionalProperties": false
                                            },
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "id": {"type": "string"},
                                                    "name": {"type": "string"},
                                                    "kind": {"const": "query"},
                                                    "sql": {"type": "string"},
                                                    "max_rows": {"type": "integer", "minimum": 1, "maximum": 1000}
                                                },
                                                "required": ["id", "name", "kind", "sql", "max_rows"],
                                                "additionalProperties": false
                                            },
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "id": {"type": "string"},
                                                    "name": {"type": "string"},
                                                    "kind": {"const": "result"}
                                                },
                                                "required": ["id", "name", "kind"],
                                                "additionalProperties": false
                                            }
                                        ]
                                    }
                                },
                                "edges": {
                                    "type": "array",
                                    "minItems": 2,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "source": {"type": "string"},
                                            "target": {"type": "string"}
                                        },
                                        "required": ["source", "target"],
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "required": ["name", "nodes", "edges"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["workflow"],
                    "additionalProperties": false
                }),
            ),
            Self::function(
                "run_query",
                "Run a bounded read-only Spiral query for inspection only.",
                json!({"type":"object","properties":{"sql":{"type":"string"},"max_rows":{"type":"integer","minimum":1,"maximum":50}},"required":["sql","max_rows"],"additionalProperties":false}),
            ),
        ]
    }

    fn function(name: &str, description: &str, parameters: Value) -> Value {
        json!({
            "type": "function",
            "function": {"name": name, "description": description, "parameters": parameters}
        })
    }
}

trait WorkflowSpecAgentExt {
    fn resolved_models_for_agent(&self, namespace: &str) -> Vec<Value>;
}

impl WorkflowSpecAgentExt for WorkflowSpec {
    fn resolved_models_for_agent(&self, namespace: &str) -> Vec<Value> {
        self.nodes()
            .iter()
            .filter_map(|node| match &node.config() {
                WorkflowNodeConfig::Model { features } => Some(json!({
                    "node_id": node.id(),
                    "model_id": "siglip-base",
                    "feature_cache_relation": features.first().map(|feature| {
                        StorageSchema::workflow_feature_cache_relation(
                            namespace,
                            feature.applicability().source_relation(),
                        )
                    }),
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

struct ToolPresentation<'a>(&'a str);

impl<'a> ToolPresentation<'a> {
    fn for_name(name: &'a str) -> Self {
        Self(name)
    }

    fn running_summary(&self) -> String {
        match self.0 {
            "list_relations" => "Inspecting Spiral relations".to_owned(),
            "describe_relations" => "Reading relation schemas".to_owned(),
            "list_model_capabilities" => "Checking model capabilities".to_owned(),
            "set_workflow" => "Validating graph".to_owned(),
            "run_query" => "Running bounded query".to_owned(),
            _ => "Using Workflow tool".to_owned(),
        }
    }

    fn display_name(&self) -> &'static str {
        match self.0 {
            "list_relations" => "Relation discovery",
            "describe_relations" => "Schema inspection",
            "list_model_capabilities" => "Model discovery",
            "set_workflow" => "Graph save",
            "run_query" => "Bounded query",
            _ => "Workflow tool",
        }
    }

    fn failure_action(&self) -> &'static str {
        match self.0 {
            "list_relations" => "find relations",
            "describe_relations" => "inspect relation fields",
            "list_model_capabilities" => "check model support",
            "set_workflow" => "save the workflow",
            "run_query" => "inspect query results",
            _ => "use the Workflow tool",
        }
    }
}

struct ToolCompletion {
    summary: String,
    receipt: Option<WorkflowAgentArtifactReceipt>,
}

impl ToolCompletion {
    fn for_tool(name: &str, ok: bool, detail: &str) -> Self {
        let value = serde_json::from_str::<Value>(detail).unwrap_or(Value::Null);
        if !ok {
            return Self {
                summary: format!(
                    "Couldn’t {}",
                    ToolPresentation::for_name(name).failure_action()
                ),
                receipt: None,
            };
        }
        match name {
            "list_relations" => Self::count(&value, "relations", "Spiral relations"),
            "describe_relations" => Self::count(&value, "relations", "relation schemas"),
            "list_model_capabilities" => Self::count(&value, "models", "model capabilities"),
            "set_workflow" => {
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
                let suffix =
                    revision.map_or_else(String::new, |revision| format!(" · r{revision}"));
                ToolCompletion {
                    summary: format!("Saved {name}{suffix}"),
                    receipt: revision.map(|revision| {
                        WorkflowAgentArtifactReceipt::graph("View graph", revision)
                    }),
                }
            }
            "run_query" => {
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
            _ => Self {
                summary: format!(
                    "{} complete",
                    ToolPresentation::for_name(name).display_name()
                ),
                receipt: None,
            },
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
            let error = agent.execute_tool(&call, &mut context).await.unwrap_err();
            assert!(error.contains("only authors"));
            assert_eq!(workflow.state(), before);
            assert!(!workflow.is_active());
        }
    }

    #[test]
    fn tool_schema_exposes_only_authoring_and_inspection() {
        let definitions = WorkflowToolCatalog::definitions();
        let names = definitions
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_relations",
                "describe_relations",
                "list_model_capabilities",
                "set_workflow",
                "run_query",
            ]
        );
        assert_eq!(
            definitions[4]["function"]["parameters"]["properties"]["max_rows"]["maximum"],
            MAX_WORKFLOW_AGENT_QUERY_ROWS
        );
        let workflow = &definitions[3]["function"]["parameters"]["properties"]["workflow"];
        assert!(workflow["properties"].get("id").is_none());
        assert!(workflow["properties"].get("revision").is_none());
        assert_eq!(workflow["required"], json!(["name", "nodes", "edges"]));
        let variants = workflow["properties"]["nodes"]["items"]["oneOf"]
            .as_array()
            .expect("set_workflow nodes have discriminated variants");
        assert_eq!(variants.len(), 4);
        assert_eq!(variants[0]["properties"]["kind"]["const"], "table");
        assert_eq!(variants[1]["properties"]["kind"]["const"], "model");
        assert_eq!(
            variants[1]["properties"]["model_id"]["const"],
            "siglip-base"
        );
        assert_eq!(variants[2]["properties"]["kind"]["const"], "query");
        assert!(variants[1]["properties"].get("sample_every_ms").is_none());
        assert!(variants[1]["properties"].get("media_column").is_none());
        assert!(variants[1]["properties"].get("concepts").is_none());
        let feature = &variants[1]["properties"]["features"]["items"];
        assert_eq!(
            feature["properties"]["name"]["pattern"],
            "^[a-z_][a-z0-9_]*_feature$"
        );
        assert_eq!(
            feature["properties"]["applicability"]["properties"]["stream_ids"]["maxItems"],
            8
        );
        assert_eq!(
            feature["properties"]["applicability"]["required"],
            json!(["source_relation", "media_field"])
        );
        assert_eq!(variants[3]["properties"]["kind"]["const"], "result");
        assert_eq!(workflow["additionalProperties"], false);
        assert!(variants[1]["properties"].get("output_relation").is_none());
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
        assert_eq!(saved.summary, "Saved Holding evidence · r7");
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
        assert_eq!(rejected.summary, "Couldn’t save the workflow");
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
    fn system_prompt_teaches_supported_sql_cohort_and_feature_cache_shape() {
        assert!(SYSTEM_PROMPT.contains("{{node:candidates}}"));
        assert!(SYSTEM_PROMPT.contains("{{node:visual_features}}"));
        assert!(SYSTEM_PROMPT.contains("{{node:joint_samples}}"));
        assert!(SYSTEM_PROMPT.contains("timeline_offset_ns"));
        assert!(SYSTEM_PROMPT.contains("actual managed media relation"));
        assert!(SYSTEM_PROMPT.contains("candidate Query → Model → final Query"));
        assert!(SYSTEM_PROMPT.contains("CROSS JOIN LATERAL spql.video.frames"));
        assert!(SYSTEM_PROMPT.contains("CAST(manifest.frames AS BIGINT)"));
        assert!(SYSTEM_PROMPT.contains("selected_frame.request_index"));
        assert!(SYSTEM_PROMPT.contains("selected output ordinal"));
        assert!(SYSTEM_PROMPT.contains(
            "CAST(selected_frame.request_index AS UINT64) * CAST(<the same stride> AS UINT64)"
        ));
        assert!(SYSTEM_PROMPT.contains("BIGINT casts are for frame_slice arguments only"));
        assert!(SYSTEM_PROMPT.contains("request_index is non-null"));
        assert!(SYSTEM_PROMPT.contains("Spiral does not support LEAD or LAG window functions"));
        assert!(SYSTEM_PROMPT.contains("LATERAL only for table functions"));
        assert!(
            SYSTEM_PROMPT.contains("next_candidate.frame_index = current_candidate.frame_index")
        );
        assert!(SYSTEM_PROMPT.contains("first inspect its actual timestamp cadence"));
        assert!(SYSTEM_PROMPT.contains("observed tolerance ns"));
        assert!(SYSTEM_PROMPT.contains("MAX(follow_error_degrees)"));
        assert!(SYSTEM_PROMPT.contains("never ranks or carries image values"));
        assert!(SYSTEM_PROMPT.contains("current_features.<feature> < 0.5"));
        assert!(SYSTEM_PROMPT.contains("a mere score increase is not a release transition"));
        assert!(SYSTEM_PROMPT.contains("at most one `spql.video.frames` generator"));
        assert!(SYSTEM_PROMPT.contains("generator output names such as request_index collide"));
        assert!(SYSTEM_PROMPT.contains("rather than wrapping decode in a lateral derived SELECT"));
        assert!(SYSTEM_PROMPT.contains("Never hide a joint-state"));
        assert!(SYSTEM_PROMPT.contains("fills only missing applicable feature cells"));
        assert!(SYSTEM_PROMPT.contains("never contains decoded images"));
        assert!(SYSTEM_PROMPT.contains("is_raining_feature"));
        assert!(SYSTEM_PROMPT.contains("Studio namespace's feature_registry"));
        assert!(SYSTEM_PROMPT.contains("description, the SigLIP text concept"));
        assert!(SYSTEM_PROMPT.contains("conflicting same-name definition is rejected"));
        assert!(SYSTEM_PROMPT.contains("only read-only Spiral functions and fields verified"));
        assert!(SYSTEM_PROMPT.contains("Never use SQL keywords such as top"));
        assert!(SYSTEM_PROMPT.contains("top_cam"));
        assert!(!SYSTEM_PROMPT.contains("collide on request_index"));
        assert!(!SYSTEM_PROMPT.contains("two decoded CTEs"));
        assert!(!SYSTEM_PROMPT.contains("sample_video_frames"));
        assert!(!SYSTEM_PROMPT.contains("sample_every_ms"));
        assert!(!SYSTEM_PROMPT.contains("pick_place."));
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
