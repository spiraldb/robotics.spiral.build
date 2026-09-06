//! Backend-owned Workflow agent, tool loop, durable session, and SSE state.

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
    DEFAULT_WORKFLOW_ID, WorkflowAgentArtifactReceipt, WorkflowAgentCommand,
    WorkflowAgentConversationSummary, WorkflowAgentMessage, WorkflowAgentMessagePhase,
    WorkflowAgentPhase, WorkflowAgentReadiness, WorkflowAgentRole, WorkflowAgentSession,
    WorkflowAgentSnapshot, WorkflowAgentToolPhase, WorkflowAgentToolSummary, WorkflowCommand,
    WorkflowNodeConfig, WorkflowRunPhase, WorkflowSpec,
};
use tokio::sync::broadcast;

use crate::workflow::manager::WorkflowManager;
use crate::workflow::provider::{
    OpenAiProvider, ProviderCompletion, ProviderConfig, ProviderResponse,
};
use spiral_robotics_storage::workflow_assistant::{
    AssistantConversation as AgentConversation, AssistantDocument as AgentDocument,
    ProviderMessage, ProviderMessageHistory, ProviderToolCall,
};
use spiral_robotics_storage::workflow_documents::{
    MAX_WORKFLOW_AGENT_CONVERSATIONS, MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION,
};
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
    document: Mutex<AgentDocument>,
    turn: AgentTurnState,
    updates: broadcast::Sender<()>,
}

struct AgentTurnState {
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
    owned_run_id: Arc<Mutex<Option<String>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

trait AgentDocumentRuntime {
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
impl AgentDocumentRuntime for AgentDocument {
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
            .conversation_mut(conversation_id)
            .expect("the active conversation was checked")
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
        let conversation = candidate
            .conversation_mut(conversation_id)
            .expect("turn was checked");
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::tool(
                call.id().to_owned(),
                detail.to_owned(),
            ));
        conversation
            .provider_messages_mut()
            .trim_to_provider_bound();
        if conversation.session().phase() == WorkflowAgentPhase::WaitingForRun {
            conversation
                .session_mut()
                .set_phase(WorkflowAgentPhase::Responding);
        }
        candidate.bump_conversation(conversation_id)?;
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
        let conversation = candidate
            .conversation_mut(conversation_id)
            .expect("the active conversation was checked");
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

trait OwnedRunTracking {
    fn record_and_is_cancelled(&self, cancel: &CancellationToken, run_id: &str) -> bool;
}

impl OwnedRunTracking for Arc<Mutex<Option<String>>> {
    fn record_and_is_cancelled(&self, cancel: &CancellationToken, run_id: &str) -> bool {
        *self.recover_lock() = Some(run_id.to_owned());
        cancel.is_cancelled()
    }
}

struct ToolContext {
    conversation_id: String,
    workflow_revision: u64,
    owned_run_id: Arc<Mutex<Option<String>>>,
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

struct ConversationChange<'a> {
    turn_active: bool,
    scope_id: &'a str,
    expected_revision: u64,
}

struct ConversationSelection {
    scope_id: String,
    conversation_id: String,
    expected_revision: u64,
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

impl<'a> ConversationChange<'a> {
    fn new(turn_active: bool, scope_id: &'a str, expected_revision: u64) -> Self {
        Self {
            turn_active,
            scope_id,
            expected_revision,
        }
    }
}

struct TurnTask {
    conversation_id: String,
    turn_id: String,
    cancel: CancellationToken,
    owned_run_id: Arc<Mutex<Option<String>>>,
}

impl TurnTask {
    fn new(conversation_id: String, turn_id: String) -> Self {
        Self {
            conversation_id,
            turn_id,
            cancel: CancellationToken::new(),
            owned_run_id: Arc::new(Mutex::new(None)),
        }
    }

    fn with_runtime(
        mut self,
        cancel: CancellationToken,
        owned_run_id: Arc<Mutex<Option<String>>>,
    ) -> Self {
        self.cancel = cancel;
        self.owned_run_id = owned_run_id;
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
                conversation_id: task.conversation_id.clone(),
                workflow_revision,
                owned_run_id: task.owned_run_id.clone(),
            },
            budget: TurnBudget::default(),
        }
    }
}

struct ToolRuntime<'a> {
    context: &'a mut ToolContext,
    cancel: CancellationToken,
    identity: TurnIdentity<'a>,
}

impl<'a> ToolRuntime<'a> {
    fn new(
        context: &'a mut ToolContext,
        cancel: CancellationToken,
        identity: TurnIdentity<'a>,
    ) -> Self {
        Self {
            context,
            cancel,
            identity,
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
        let (updates, _) = broadcast::channel(128);
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
                document: Mutex::new(AgentDocument::default()),
                turn: AgentTurnState {
                    active: Mutex::new(None),
                    mutation: tokio::sync::Mutex::new(()),
                    recovery_pending: AtomicBool::new(false),
                },
                updates,
            }),
        }
    }

    /// Candidate preparation validates history without rewriting it or starting a turn.
    pub(crate) async fn prepare_from_storage(&self) -> Result<(), String> {
        let stored = self.stored_document().await?;
        let (document, rewrite) = AgentDocument::restored_snapshot(
            stored.as_deref(),
            "Studio reconnected before this Assistant turn finished. Retry the turn.",
        )?;
        *self.inner.document.recover_lock() = document;
        self.inner
            .turn
            .recovery_pending
            .store(rewrite, Ordering::SeqCst);
        self.publish();
        Ok(())
    }

    /// Read the persisted Assistant payload, refusing a document version this
    /// build cannot read instead of replacing it.
    async fn stored_document(&self) -> Result<Option<String>, String> {
        self.inner
            .services
            .storage
            .load_workflow_agent_document()
            .await?
            .as_ref()
            .map(|stored| AgentDocument::supported_payload(stored).map(str::to_owned))
            .transpose()
    }

    pub(crate) async fn persist_recovery(&self) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        if self.inner.turn.recovery_pending.load(Ordering::SeqCst) {
            let document = self.inner.document.recover_lock().clone();
            self.persist_document(&document).await?;
            self.inner
                .turn
                .recovery_pending
                .store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    pub(crate) fn state(&self) -> WorkflowAgentSnapshot {
        let document = self.inner.document.recover_lock();
        self.state_for(&document)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.turn.active.recover_lock().is_some()
    }

    pub(crate) fn subscription(&self) -> (WorkflowAgentSnapshot, broadcast::Receiver<()>) {
        let document = self.inner.document.recover_lock();
        let updates = self.inner.updates.subscribe();
        (self.state_for(&document), updates)
    }

    pub(crate) async fn command(
        &self,
        command: WorkflowAgentCommand,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        match command {
            WorkflowAgentCommand::CreateConversation {
                scope_id,
                expected_revision,
            } => self.create_conversation(&scope_id, expected_revision).await,
            WorkflowAgentCommand::ActivateConversation {
                scope_id,
                conversation_id,
                expected_revision,
            } => {
                self.activate_conversation(ConversationSelection {
                    scope_id,
                    conversation_id,
                    expected_revision,
                })
                .await
            }
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

    async fn create_conversation(
        &self,
        scope_id: &str,
        expected_revision: u64,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let candidate = {
            let active = self.inner.turn.active.recover_lock();
            let document = self.inner.document.recover_lock();
            self.admit_conversation_change(
                &document,
                ConversationChange::new(active.is_some(), scope_id, expected_revision),
            )?;
            let Some(candidate) = document
                .with_new_conversation(AgentClock::now_millis())
                .map_err(OperationError::Conflict)?
            else {
                return Ok(self.state_for(&document));
            };
            candidate
        };
        self.persist_document(&candidate)
            .await
            .map_err(OperationError::External)?;
        let state = self.state_for(&candidate);
        *self.inner.document.recover_lock() = candidate;
        self.publish();
        Ok(state)
    }

    async fn activate_conversation(
        &self,
        selection: ConversationSelection,
    ) -> Result<WorkflowAgentSnapshot, OperationError> {
        let ConversationSelection {
            scope_id,
            conversation_id,
            expected_revision,
        } = selection;
        let _mutation = self.inner.turn.mutation.lock().await;
        let candidate = {
            let active = self.inner.turn.active.recover_lock();
            let document = self.inner.document.recover_lock();
            self.admit_conversation_change(
                &document,
                ConversationChange::new(active.is_some(), &scope_id, expected_revision),
            )?;
            if document.conversation(&conversation_id).is_none() {
                return Err(OperationError::InvalidInput(
                    "That Assistant conversation no longer exists. Reload the conversation list."
                        .to_owned(),
                ));
            }
            let Some(candidate) = document
                .activated_conversation(&conversation_id)
                .map_err(OperationError::External)?
            else {
                return Ok(self.state_for(&document));
            };
            candidate
        };
        self.persist_document(&candidate)
            .await
            .map_err(OperationError::External)?;
        let state = self.state_for(&candidate);
        *self.inner.document.recover_lock() = candidate;
        self.publish();
        Ok(state)
    }

    fn admit_conversation_change(
        &self,
        document: &AgentDocument,
        change: ConversationChange<'_>,
    ) -> Result<(), OperationError> {
        let ConversationChange {
            turn_active,
            scope_id,
            expected_revision,
        } = change;
        if !self.inner.services.storage.is_configured() {
            return Err(OperationError::Conflict(
                "Configure or reconnect Spiral before changing Assistant conversations.".to_owned(),
            ));
        }
        if scope_id != *self.inner.scope_id.recover_lock() {
            return Err(OperationError::Conflict(
                "The active Spiral project changed. Reload Assistant conversations.".to_owned(),
            ));
        }
        if expected_revision != document.revision() {
            return Err(OperationError::Conflict(
                "Assistant conversations changed in another session. Reload them first.".to_owned(),
            ));
        }
        if turn_active {
            return Err(OperationError::Conflict(
                "Stop the active Assistant turn before changing conversations.".to_owned(),
            ));
        }
        let workflow_state = self.inner.services.workflow.state();
        let workflow = workflow_state.snapshot();
        if workflow.phase().active()
            && workflow.run_id().is_some_and(|run_id| {
                document
                    .conversations()
                    .iter()
                    .any(|conversation| conversation.owned_run_id() == Some(run_id))
            })
        {
            return Err(OperationError::Conflict(
                "Stop the Workflow run started by the Assistant before changing conversations."
                    .to_owned(),
            ));
        }
        Ok(())
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
                let document = self.inner.document.recover_lock();
                if scope_id != *self.inner.scope_id.recover_lock()
                    || conversation_id != document.active_conversation_id()
                {
                    return Err(OperationError::Conflict(
                        "The active Assistant conversation changed. Reload it before sending."
                            .to_owned(),
                    ));
                }
                if expected_revision != document.revision() {
                    return Err(OperationError::Conflict(
                        "Assistant history changed in another session. Reload it before sending."
                            .to_owned(),
                    ));
                }
                let conversation = document.active().ok_or_else(|| {
                    OperationError::External(
                        "The selected Assistant conversation is unavailable.".to_owned(),
                    )
                })?;
                PublicMessageCapacity::new(conversation.session().messages().len())
                    .ensure_additional(2)
                    .map_err(OperationError::Conflict)?;
                (
                    document.clone(),
                    document.active_conversation_id().to_owned(),
                )
            };
            let turn_id = AgentIdentifier::next("agent-turn");
            let user_id = AgentIdentifier::next("agent-user");
            let assistant_id = AgentIdentifier::next("agent-assistant");
            let created_ms = AgentClock::now_millis();
            let conversation = candidate
                .conversation_mut(&conversation_id)
                .expect("the selected conversation was checked");
            if conversation.title() == "New conversation"
                && !conversation
                    .session()
                    .messages()
                    .iter()
                    .any(|message| message.role() == WorkflowAgentRole::User)
            {
                conversation.set_title(AgentConversation::title_from_user_text(text));
            }
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
            conversation.session_mut().set_deferred_run_id(None);
            conversation.session_mut().set_error(None);
            candidate
                .bump_conversation(&conversation_id)
                .map_err(OperationError::External)?;
            candidate.trim_provider_history(&conversation_id);
            self.persist_document(&candidate)
                .await
                .map_err(OperationError::External)?;
            let cancel = CancellationToken::new();
            let owned_run_id = Arc::new(Mutex::new(None));
            let response = self.state_for(&candidate);
            *self.inner.document.recover_lock() = candidate;
            *self.inner.turn.active.recover_lock() = Some(ActiveTurn {
                conversation_id: conversation_id.clone(),
                turn_id: turn_id.clone(),
                cancel: cancel.clone(),
                owned_run_id: owned_run_id.clone(),
                task: None,
            });
            self.publish();
            let manager = self.clone();
            let task_conversation_id = conversation_id.clone();
            let task_turn_id = turn_id.clone();
            let turn_task = TurnTask::new(task_conversation_id, task_turn_id)
                .with_runtime(cancel, owned_run_id);
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
        let (conversation_id, turn_id, owned_run_id, task) = {
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
                active.owned_run_id.clone(),
                active.task.take(),
            )
        };
        {
            let _mutation = self.inner.turn.mutation.lock().await;
            let mut candidate = self.inner.document.recover_lock().clone();
            if let Some(conversation) = candidate.conversation_mut(&conversation_id)
                && conversation.session().active_turn_id() == Some(turn_id.as_str())
            {
                conversation
                    .session_mut()
                    .set_phase(WorkflowAgentPhase::Cancelling);
                conversation.session_mut().set_error(None);
                if candidate.bump_conversation(&conversation_id).is_ok()
                    && self.persist_document(&candidate).await.is_ok()
                {
                    *self.inner.document.recover_lock() = candidate;
                }
            }
        }
        self.publish();
        self.cancel_owned_run(&owned_run_id).await;
        if let Some(task) = task {
            let _ = task.await;
        }
        // `run_workflow` may have started after the first snapshot but before
        // observing the turn token. Re-read the synchronized ownership cell
        // after the task exits so Stop cannot orphan that run.
        self.cancel_owned_run(&owned_run_id).await;
        self.finish_failed(
            TurnIdentity::new(&conversation_id, &turn_id),
            TurnFailureDetail::new("Agent turn cancelled.", true),
        )
        .await;
        Ok(self.state())
    }

    async fn cancel_owned_run(&self, owned_run_id: &Arc<Mutex<Option<String>>>) {
        let owned_run = owned_run_id.recover_lock().clone();
        if let Some(run_id) = owned_run
            && self.inner.services.workflow.state().snapshot().run_id() == Some(run_id.as_str())
            && self.inner.services.workflow.is_active()
        {
            let _ = self
                .inner
                .services
                .workflow
                .command(WorkflowCommand::cancel(
                    self.inner.services.workflow.scope_id(),
                ))
                .await;
        }
    }

    async fn record_conversation_owned_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let mut candidate = self.inner.document.recover_lock().clone();
        let conversation = candidate
            .conversation_mut(conversation_id)
            .ok_or_else(|| "Assistant conversation disappeared.".to_owned())?;
        conversation.set_owned_run_id(Some(run_id.to_owned()));
        candidate.bump_conversation(conversation_id)?;
        self.persist_document(&candidate).await?;
        *self.inner.document.recover_lock() = candidate;
        Ok(())
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
        let workflow_revision = self.inner.services.workflow.state().workflow().revision();
        let identity = TurnIdentity::new(&task.conversation_id, &task.turn_id);
        match self
            .run_provider_loop(ProviderLoop::new(&task, workflow_revision))
            .await
        {
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
                    .document
                    .recover_lock()
                    .conversation(conversation_id)
                    .ok_or_else(|| "Assistant conversation disappeared.".to_owned())?
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
                let result = self
                    .execute_tool(
                        call,
                        ToolRuntime::new(&mut context, cancel.clone(), identity),
                    )
                    .await;
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
        runtime: ToolRuntime<'_>,
    ) -> Result<String, String> {
        let ToolRuntime {
            context,
            cancel,
            identity,
        } = runtime;
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
                    Value::String(DEFAULT_WORKFLOW_ID.to_owned()),
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
                        self.inner.services.storage.namespace()
                    ),
                })
                .to_string())
            }
            "run_workflow" => {
                let lease = self
                    .inner
                    .services
                    .catalog_lease
                    .try_acquire(CatalogLeaseOwner::WorkflowStart)
                    .ok_or_else(|| {
                        "A recording or catalog change currently prevents Workflow start."
                            .to_owned()
                    })?;
                let result = self
                    .inner
                    .services
                    .workflow
                    .command(WorkflowCommand::run(
                        self.inner.services.workflow.scope_id(),
                        context.workflow_revision,
                    ))
                    .await
                    .map_err(|error| error.to_string());
                let released = self.inner.services.catalog_lease.release(lease);
                debug_assert!(released, "agent Workflow start releases its lease once");
                let state = result?;
                let snapshot = state.snapshot();
                let run_id = snapshot
                    .run_id()
                    .ok_or_else(|| "Workflow start returned no run identity.".to_owned())?
                    .to_owned();
                let cancelled = context
                    .owned_run_id
                    .record_and_is_cancelled(&cancel, &run_id);
                if let Err(error) = self
                    .record_conversation_owned_run(&context.conversation_id, &run_id)
                    .await
                {
                    if self.inner.services.workflow.state().snapshot().run_id() == Some(&run_id)
                        && self.inner.services.workflow.is_active()
                    {
                        let _ = self
                            .inner
                            .services
                            .workflow
                            .command(WorkflowCommand::cancel(
                                self.inner.services.workflow.scope_id(),
                            ))
                            .await;
                    }
                    return Err(format!(
                        "Could not persist Assistant ownership of the Workflow run: {error}"
                    ));
                }
                if cancelled {
                    if self.inner.services.workflow.state().snapshot().run_id() == Some(&run_id)
                        && self.inner.services.workflow.is_active()
                    {
                        let _ = self
                            .inner
                            .services
                            .workflow
                            .command(WorkflowCommand::cancel(
                                self.inner.services.workflow.scope_id(),
                            ))
                            .await;
                    }
                    return Err(
                        "Agent turn was cancelled after starting the Workflow run.".to_owned()
                    );
                }
                self.mark_waiting(identity, &run_id).await?;
                Ok(self
                    .inner
                    .services
                    .workflow
                    .wait_for_terminal(&run_id, cancel)
                    .await?
                    .agent_summary()
                    .to_string())
            }
            "wait_for_run" => {
                let run_id = context
                    .owned_run_id
                    .recover_lock()
                    .clone()
                    .or_else(|| {
                        self.inner
                            .services
                            .workflow
                            .state()
                            .snapshot()
                            .run_id()
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| "There is no Workflow run to wait for.".to_owned())?;
                self.mark_waiting(identity, &run_id).await?;
                Ok(self
                    .inner
                    .services
                    .workflow
                    .wait_for_terminal(&run_id, cancel)
                    .await?
                    .agent_summary()
                    .to_string())
            }
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
        let mut document = self.inner.document.recover_lock();
        let Some(conversation) = document.conversation_mut(conversation_id) else {
            return;
        };
        if conversation.session().active_turn_id() != Some(turn_id) {
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
        let _ = document.bump_conversation(conversation_id);
        drop(document);
        self.publish();
    }

    async fn record_provider_response(
        &self,
        identity: TurnIdentity<'_>,
        message: ProviderMessage,
    ) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let candidate = self
            .inner
            .document
            .recover_lock()
            .with_provider_message(identity, message)?;
        self.persist_document(&candidate).await?;
        *self.inner.document.recover_lock() = candidate;
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
        let mut candidate = self.inner.document.recover_lock().clone();
        candidate.ensure_turn(conversation_id, turn_id)?;
        candidate
            .conversation_mut(conversation_id)
            .expect("the active conversation was checked")
            .provider_messages_mut()
            .append_unexecuted_tool_outputs(unexecuted);
        candidate.trim_provider_history(conversation_id);
        self.persist_document(&candidate).await?;
        *self.inner.document.recover_lock() = candidate;
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
        let mut document = self.inner.document.recover_lock();
        document.ensure_turn(conversation_id, turn_id)?;
        let message = document
            .conversation_mut(conversation_id)
            .expect("the active conversation was checked")
            .session_mut()
            .current_assistant_mut()
            .ok_or_else(|| "Assistant message disappeared during tool execution.".to_owned())?;
        message.tools_mut().push(WorkflowAgentToolSummary::new(
            call.id(),
            call.function().name(),
            ToolPresentation::for_name(call.function().name()).running_summary(),
        ));
        document.bump_conversation(conversation_id)?;
        drop(document);
        self.publish();
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
            .document
            .recover_lock()
            .with_completed_tool(identity, completion)?;
        self.persist_document(&candidate).await?;
        *self.inner.document.recover_lock() = candidate;
        self.publish();
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
        let mut candidate = self.inner.document.recover_lock().clone();
        let Some(conversation) = candidate.conversation_mut(conversation_id) else {
            return;
        };
        if conversation.session().active_turn_id() != Some(turn_id) {
            return;
        }
        conversation.session_mut().set_phase(phase);
        conversation.session_mut().set_error(error);
        conversation.session_mut().set_active_turn_id(None);
        conversation.session_mut().set_deferred_run_id(None);
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
        let _ = candidate.bump_conversation(conversation_id);
        if let Err(persist_error) = self.persist_document(&candidate).await
            && let Some(conversation) = candidate.conversation_mut(conversation_id)
        {
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
        // live document is never left with an inactive orphaned tool exchange.
        *self.inner.document.recover_lock() = candidate;
        *self.inner.turn.active.recover_lock() = None;
        self.publish();
    }

    async fn mark_waiting(&self, identity: TurnIdentity<'_>, run_id: &str) -> Result<(), String> {
        let _mutation = self.inner.turn.mutation.lock().await;
        let mut candidate = self.inner.document.recover_lock().clone();
        candidate.ensure_turn(identity.conversation_id, identity.turn_id)?;
        let session = candidate
            .conversation_mut(identity.conversation_id)
            .expect("turn was checked")
            .session_mut();
        if session.phase() != WorkflowAgentPhase::Cancelling {
            session.set_phase(WorkflowAgentPhase::WaitingForRun);
        }
        if let Some(message) = session.current_assistant_mut() {
            message.set_run_id(Some(run_id.to_owned()));
        }
        candidate.bump_conversation(identity.conversation_id)?;
        self.persist_document(&candidate).await?;
        *self.inner.document.recover_lock() = candidate;
        self.publish();
        Ok(())
    }

    fn state_for(&self, document: &AgentDocument) -> WorkflowAgentSnapshot {
        let session = document
            .active()
            .map(|conversation| conversation.session().clone())
            .unwrap_or_default();
        WorkflowAgentSnapshot::new(self.inner.readiness.clone(), session)
            .with_history_available(true)
            .with_scope(
                self.inner.scope_id.recover_lock().clone(),
                document.revision(),
            )
            .with_active_conversation(
                document.active_conversation_id().to_owned(),
                MAX_WORKFLOW_AGENT_CONVERSATIONS,
            )
            .with_conversations(
                document
                    .conversations()
                    .iter()
                    .map(|conversation| {
                        WorkflowAgentConversationSummary::new(
                            conversation.conversation_id().to_owned(),
                            conversation.title().to_owned(),
                            conversation.created_ms(),
                        )
                        .with_activity(
                            conversation.updated_ms(),
                            conversation.session().messages().len(),
                        )
                        .with_owned_run_id(conversation.owned_run_id().map(str::to_owned))
                    })
                    .collect(),
            )
    }

    async fn persist_document(&self, document: &AgentDocument) -> Result<(), String> {
        self.inner
            .services
            .storage
            .save_workflow_agent_document(&document.persisted()?)
            .await
    }

    fn publish(&self) {
        let _ = self.inner.updates.send(());
    }
}

const SYSTEM_PROMPT: &str = r#"You are the Workflow Assistant in Spiral Robotics Studio. Work only with relations already materialized in the active Spiral project. Never import Hugging Face datasets or MCAP files. Build relation-first DAGs using Table, optional siglip-base Model, Query, and exactly one terminal Result. Name workflows and nodes for the user's intent. Query SQL is read-only. Model output relations and feature columns are resolved by the server. Use tools to inspect authoritative schemas, validate and save the graph, run it, wait, and inspect bounded results.

SigLIP enrichment operates on a SQL-defined candidate cohort. The graph shape is source Table → candidate Query → Model → final Query → Result. Also connect the candidate Query directly to the final Query so it can join candidate identity to the Model feature cache. Other evidence such as a joint-samples Table joins only at the final Query. Reference every connected upstream node through its exact placeholder, for example {{node:candidates}}, {{node:visual_features}}, and {{node:joint_samples}}. Every ordinary durable relation used by SQL must be represented by a connected Table node; the only exception is the actual managed media relation needed to resolve a manifest's media value. Never hide a joint-state or other scalar relation inside candidate SQL: add it as a Table root, join it in the final Query, apply its predicates there, and return the matching scalar evidence in Results. A Model must trace to exactly one authoritative source Table; the server derives one cross-workflow feature-cache relation from that source identity.

The candidate Query must return non-null episode_id Utf8, stream_id Utf8, segment_index UInt32, frame_index UInt64, episode_time_ns UInt64, and every decoded spql.image.Image media_field named by its feature definitions. Author periodic or downsampled cohorts in this read-only SQL, not as Model configuration and not through a mutation tool. An inspected durable video manifest may expose episode_id, stream_id, segment_index, timeline_offset_ns, fps, frames, and media_relation. Use the actual managed media relation discovered with tools. Spiral frame selection happens before decode. Join the manifest row to that managed media relation, then use the table-function form `CROSS JOIN LATERAL spql.video.frames(asset.media, spql.video.frame_slice(CAST(0 AS BIGINT), CAST(manifest.frames AS BIGINT), CAST(<positive integral stride derived from manifest.fps and the requested cadence> AS BIGINT))) AS selected_frame`. Read `selected_frame.request_index` and `selected_frame.frame`; decode the latter with `spql.video.decode`. request_index is the zero-based selected output ordinal, not the source video frame number. For a zero-start fixed stride, emit the source identity as `CAST(selected_frame.request_index AS UINT64) * CAST(<the same stride> AS UINT64) AS frame_index`, and use that source frame index in the episode_time_ns calculation. request_index is non-null, so never wrap it in COALESCE. BIGINT casts are for frame_slice arguments only: keep segment_index as UInt32 and derive frame_index and episode_time_ns with UINT64 operands/casts so the five returned identity fields retain their required unsigned types. The pinned backend processor owns bicubic SigLIP resizing.

Spiral does not support LEAD or LAG window functions, and it supports LATERAL only for table functions such as `spql.video.frames`, not for derived SELECT subqueries. To find the sampled frame immediately before a visual transition, use ordinary self-joins over the candidate and feature placeholders: join the next candidate on the same episode_id, stream_id, and segment_index with `next_candidate.frame_index = current_candidate.frame_index + CAST(<the same fixed sampling stride used by frame_slice> AS UINT64)`, then join the feature cache once for the current key and once for the next key. Unless the user specifies another calibrated boundary, define the transition as `current_features.<feature> < 0.5 AND next_features.<feature> >= 0.5`; a mere score increase is not a release transition and can make evidence recovery unbounded. For nearby joint-state evidence, first inspect its actual timestamp cadence, then use a narrow bounded temporal join such as `joint_state.episode_time_ns + CAST(<observed tolerance ns> AS UINT64) >= current_candidate.episode_time_ns AND joint_state.episode_time_ns <= current_candidate.episode_time_ns + CAST(<observed tolerance ns> AS UINT64)`. Aggregate the scalar evidence by the stable candidate identity (for example MAX(follow_error_degrees)) before joining decoded images, so the engine never ranks or carries image values across an expanded all-prior-rows join. Decode a selected table-function frame directly in the SELECT list rather than wrapping decode in a lateral derived SELECT. Do not emit LEAD, LAG, or a lateral derived subquery.

Use at most one `spql.video.frames` generator in each SELECT scope because generator output names such as request_index collide even when aliases differ. For paired-camera evidence, carry the candidate Query's already-decoded primary image into the final Query and use one frames generator only for the second camera. If both images must be decoded independently, isolate each generator in its own CTE and join the two scalar-keyed CTEs afterward.

Write SQL identifiers conservatively. Never use SQL keywords such as top, table, result, query, select, group, order, or window as unquoted relation, CTE, or table aliases; use descriptive aliases such as top_cam, gripper_cam, candidate_frames, and joint_state instead. Before saving a graph, inspect the complete generated SQL for reserved-word aliases and replace them.

Define every requested Model feature explicitly with a safe descriptive name ending in _feature, a short human description, the SigLIP text concept, and applicability containing the source relation, candidate media field, and optional stream_ids. Inspect the Studio namespace's feature_registry relation with the ordinary relation/query tools before introducing a name: a compatible source-scoped definition is reused across workflows, while a conflicting same-name definition is rejected. The Model cache contains only the five stable identity fields plus nullable feature columns such as is_raining_feature or is_gripper_open_feature; it never contains decoded images. The backend reuses matching cells across workflows over the same source and fills only missing applicable feature cells. A final Query joins the candidate cohort, feature cache, and optional joint state, then may recover full-resolution image evidence from an actual managed media relation using only read-only Spiral functions and fields verified through the relation tools.

Never invent relations, columns, managed media values, model support, run state, or SPQL functions. Do not request or reveal provider credentials. Do not put binary, tensor, image, or full media payloads in the conversation. Keep final answers concise and structured: state what graph was saved or run, summarize bounded evidence, and point the user to the authoritative graph or Results."#;

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
                "run_workflow",
                "Run the current saved Workflow revision and wait for its durable completion. Progress remains visible while this tool is pending.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            Self::function(
                "wait_for_run",
                "Wait for the current Workflow run to reach its durable terminal state.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
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

trait WorkflowRunAgentSummary {
    fn agent_summary(&self) -> Value;
}

impl WorkflowRunAgentSummary for spiral_robotics_common::workflow::WorkflowRunSnapshot {
    fn agent_summary(&self) -> Value {
        json!({
            "ok": self.phase() == WorkflowRunPhase::Completed,
            "run_id": self.run_id(),
            "phase": self.phase(),
            "error": self.error(),
            "result_stale": self.result_stale(),
            "result": self.result().as_ref().filter(|_| {
                self.phase() == WorkflowRunPhase::Completed && !self.result_stale()
            }).map(|result| json!({
                "columns": result.columns(),
                "row_count": result.rows().len(),
                "truncated": result.truncated(),
                "elapsed_ms": result.elapsed_ms(),
            })),
        })
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
            "run_workflow" => "Running Workflow".to_owned(),
            "wait_for_run" => "Waiting for Workflow".to_owned(),
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
            "run_workflow" => "Workflow run",
            "wait_for_run" => "Workflow wait",
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
            "run_workflow" => "run the workflow",
            "wait_for_run" => "finish waiting for the run",
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
            if matches!(name, "run_workflow" | "wait_for_run") && value.get("run_id").is_some() {
                return Self::wait(&value);
            }
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
            "run_workflow" | "wait_for_run" => Self::wait(&value),
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

    fn wait(value: &Value) -> Self {
        let run_id = ToolPublicText::optional_label(value.get("run_id").and_then(Value::as_str));
        let phase = value
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("running");
        let row_count = value
            .get("result")
            .and_then(|result| result.get("row_count"))
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok());
        let terminal_with_results = phase == "completed";
        let summary = if terminal_with_results {
            match (run_id.as_deref(), row_count) {
                (Some(run_id), Some(rows)) => format!("Completed {run_id} · {rows} rows"),
                (Some(run_id), None) => format!("Completed {run_id}"),
                (None, Some(rows)) => format!("Workflow complete · {rows} rows"),
                (None, None) => "Workflow complete".to_owned(),
            }
        } else {
            let phase = WorkflowRunPresentation::phase(phase);
            run_id.as_deref().map_or_else(
                || format!("Workflow {phase}"),
                |run_id| format!("{run_id} · {phase}"),
            )
        };
        let receipt = run_id.map(|run_id| {
            if terminal_with_results {
                WorkflowAgentArtifactReceipt::results("View results", run_id, row_count)
            } else {
                WorkflowAgentArtifactReceipt::run("View run", run_id)
            }
        });
        Self { summary, receipt }
    }
}

struct WorkflowRunPresentation;

impl WorkflowRunPresentation {
    fn phase(phase: &str) -> &str {
        match phase {
            "validating" => "validating",
            "running" => "running",
            "cancelling" => "cancelling",
            "completed" => "complete",
            "failed" => "failed",
            "cancelled" => "cancelled",
            "interrupted" => "interrupted",
            _ => "updated",
        }
    }
}

struct ToolPublicText;

impl ToolPublicText {
    fn optional_label(value: Option<&str>) -> Option<String> {
        value.map(Self::label).filter(|value| !value.is_empty())
    }

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

    #[test]
    fn tool_schema_exposes_only_the_seven_typed_tools() {
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
                "run_workflow",
                "wait_for_run",
                "run_query",
            ]
        );
        assert_eq!(
            definitions[6]["function"]["parameters"]["properties"]["max_rows"]["maximum"],
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
        assert!(matches!(
            &receipt,
            WorkflowAgentArtifactReceipt::Graph { .. }
        ));
        assert_eq!(receipt.revision(), Some(7));
        assert!(!serde_json::to_string(&receipt).unwrap().contains("secret"));

        let waited = ToolCompletion::for_tool(
            "wait_for_run",
            true,
            &json!({
                "ok": true,
                "run_id": "run-42",
                "phase": "completed",
                "result": {"row_count": 0}
            })
            .to_string(),
        );
        assert_eq!(waited.summary, "Completed run-42 · 0 rows");
        let receipt = waited.receipt.expect("terminal result receipt");
        assert!(matches!(
            &receipt,
            WorkflowAgentArtifactReceipt::Results { .. }
        ));
        assert_eq!(receipt.run_id(), Some("run-42"));
        assert_eq!(receipt.row_count(), Some(0));

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
        let mut document = AgentDocument::default();
        let conversation_id = document.active_conversation_id().to_owned();
        let conversation = document.conversation_mut(&conversation_id).unwrap();
        conversation
            .session_mut()
            .set_phase(WorkflowAgentPhase::Responding);
        conversation
            .session_mut()
            .set_active_turn_id(Some("turn-1".to_owned()));
        conversation
            .provider_messages_mut()
            .push(ProviderMessage::text("user", "inspect relations"));

        let candidate = document
            .with_provider_message(
                TurnIdentity::new(&conversation_id, "turn-1"),
                ProviderMessage::text("assistant", "I found them."),
            )
            .unwrap();

        assert_eq!(
            document
                .conversation(&conversation_id)
                .unwrap()
                .provider_messages()
                .len(),
            1,
            "a failed candidate persistence must leave the live history unchanged"
        );
        assert_eq!(
            candidate
                .conversation(&conversation_id)
                .unwrap()
                .provider_messages()
                .len(),
            2
        );
    }

    #[test]
    fn tool_completion_commits_public_receipt_and_provider_result_in_one_candidate() {
        for phase in [
            WorkflowAgentPhase::WaitingForRun,
            WorkflowAgentPhase::Cancelling,
        ] {
            let mut document = AgentDocument::default();
            let conversation_id = document.active_conversation_id().to_owned();
            let call = ProviderToolCall::new("call-1", "run_workflow", "{}");
            let conversation = document.conversation_mut(&conversation_id).unwrap();
            conversation.session_mut().set_phase(phase);
            conversation
                .session_mut()
                .set_active_turn_id(Some("turn-1".to_owned()));
            conversation.session_mut().messages_mut().push(
                WorkflowAgentMessage::new("assistant-1", WorkflowAgentRole::Assistant)
                    .with_phase(WorkflowAgentMessagePhase::Streaming)
                    .with_tools(vec![WorkflowAgentToolSummary::new(
                        "call-1",
                        "run_workflow",
                        "Running",
                    )]),
            );
            conversation
                .provider_messages_mut()
                .push(ProviderMessage::assistant(None, vec![call.clone()]));
            let detail =
                json!({"ok": true, "phase": "completed", "run_id": "run-1", "result": {"row_count": 3}})
                    .to_string();
            let candidate = document
                .with_completed_tool(
                    TurnIdentity::new(&conversation_id, "turn-1"),
                    ToolRunCompletion::new(&call, true, &detail),
                )
                .unwrap();
            let original = document.conversation(&conversation_id).unwrap();
            assert_eq!(original.provider_messages().len(), 1);
            assert_eq!(
                original.session().messages()[0].tools()[0].phase(),
                WorkflowAgentToolPhase::Running
            );
            let completed = candidate.conversation(&conversation_id).unwrap();
            assert_eq!(completed.provider_messages().len(), 2);
            assert_eq!(
                completed.provider_messages()[1].tool_call_id(),
                Some("call-1")
            );
            assert_eq!(
                completed.session().messages()[0].tools()[0].phase(),
                WorkflowAgentToolPhase::Complete
            );
            assert!(matches!(
                completed.session().messages()[0].tools()[0].receipt(),
                Some(WorkflowAgentArtifactReceipt::Results {
                    row_count: Some(3),
                    ..
                })
            ));
            assert_eq!(
                completed.session().phase(),
                if phase == WorkflowAgentPhase::WaitingForRun {
                    WorkflowAgentPhase::Responding
                } else {
                    phase
                }
            );
            assert!(candidate.revision() > document.revision());
            assert!(candidate.persisted().is_ok());
            assert!(
                document
                    .with_completed_tool(
                        TurnIdentity::new(&conversation_id, "stale-turn"),
                        ToolRunCompletion::new(&call, true, &detail)
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn run_identity_assigned_after_stop_remains_visible_to_cancellation() {
        let owned_run_id = Arc::new(Mutex::new(None));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(owned_run_id.record_and_is_cancelled(&cancel, "run-after-stop"));
        assert_eq!(
            owned_run_id.recover_lock().as_deref(),
            Some("run-after-stop")
        );
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
