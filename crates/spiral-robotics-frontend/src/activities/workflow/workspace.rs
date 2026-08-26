//! Four-panel relation Workflow editor, execution canvas, and real results.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Flow, FlowHandle, Handle, HandleKind, LayoutDirection,
    LayoutOptions, NodeViewCtx, Point, use_flow_handle,
};
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_api::workflow::{
    WorkflowAgentConversationSummary, WorkflowAgentMessagePhase, WorkflowAgentPhase,
    WorkflowAgentRole, WorkflowAgentToolPhase, WorkflowAgentToolSummary, WorkflowNodeConfig,
    WorkflowNodeKind, WorkflowNodePhase, WorkflowRunPhase,
};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AcceptedAgentDraft, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID,
    ModelFeatureTarget, NODE_PANEL_ID, RESULTS_PANEL_ID, WorkflowNodeData, WorkflowPendingCommand,
    workflow_controller,
};
use crate::components::icons::{
    FitViewIcon, ModelNodeIcon, QueryNodeIcon, ResultNodeIcon, TableNodeIcon, WorkflowIcon,
};
use crate::shell::state::studio;

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-relation-canvas-v3";
const WORKFLOW_MIN_ZOOM: f64 = 0.70;
const WORKFLOW_MAX_ZOOM: f64 = 1.8;

#[derive(Clone, Debug, Eq, PartialEq)]
enum AssistantBlock {
    Paragraph(Vec<AssistantInline>),
    List(Vec<Vec<AssistantInline>>),
    Code {
        language: Option<String>,
        text: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AssistantInline {
    Text(String),
    Strong(String),
    Code(String),
}

#[component]
pub(crate) fn WorkflowWorkspace() -> Element {
    let controller = workflow_controller();
    let panels = vec![
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-left",
            rsx! { WorkflowAssistant {} },
        )
        .with_class("workflow-assistant-editor"),
        Panel::new(
            NODE_PANEL_ID,
            "Node",
            "workflow-left",
            rsx! { WorkflowNodeInspector {} },
        )
        .with_class("workflow-node-editor"),
        Panel::new(
            CANVAS_PANEL_ID,
            "Workflow",
            "workflow-canvas",
            rsx! { WorkflowCanvas {} },
        )
        .with_class("workflow-canvas-editor"),
        Panel::new(
            RESULTS_PANEL_ID,
            "Results",
            "workflow-results",
            rsx! { WorkflowResults {} },
        )
        .with_class("workflow-results-editor"),
    ];

    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: WorkflowWorkspaceLayout::initial(),
            layout_scope: Some(WORKFLOW_LAYOUT_SCOPE.to_owned()),
            active_panel: controller.active_panel(),
            on_layout_change: move |_layout: PanelLayout| {},
            on_panel_activate: move |id: PanelId| controller.activate_panel(id),
            on_resize: move |_| {},
        }
    }
}

struct WorkflowWorkspaceLayout;

impl WorkflowWorkspaceLayout {
    fn initial() -> PanelLayout {
        let right = LayoutNode::split(
            "workflow-right-rows",
            SplitAxis::Vertical,
            0.66,
            LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
            LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
        );
        PanelLayout::new(LayoutNode::split(
            "workflow-columns",
            SplitAxis::Horizontal,
            0.30,
            LayoutNode::tile("workflow-left", [ASSISTANT_PANEL_ID, NODE_PANEL_ID]),
            right,
        ))
    }
}

#[component]
fn WorkflowAssistant() -> Element {
    let controller = workflow_controller();
    let studio = studio();
    let state = controller.agent_state();
    let mut follow_latest = use_signal(|| true);
    let mut show_latest_action = use_signal(|| false);
    let mut thread_tail = use_signal(|| None::<Rc<MountedData>>);
    let mut drafts = controller.agent_drafts();
    let mut accepted_draft = controller.agent_accepted_draft();
    use_effect(move || {
        let accepted = accepted_draft.read().to_owned();
        if let Some(accepted) = accepted {
            accepted.clear_from(&mut drafts.write());
            accepted_draft.set(None);
        }
    });
    let command = controller.agent_command();
    let active = state.session().phase().active();
    let configured = state.readiness().configured();
    let conversation_id = state.active_conversation_id().to_owned();
    let draft_key = AgentDraftKey::new(state.scope_id(), &conversation_id);
    let draft = drafts.read().get(&draft_key).cloned().unwrap_or_default();
    let catalog_available = studio.snapshot().read().readiness().catalog_available();
    let conversation_reason =
        controller.agent_conversation_disabled_reason((studio.live())(), catalog_available);
    let new_conversation_reason =
        controller.new_agent_conversation_disabled_reason((studio.live())(), catalog_available);
    let workflow_active = controller.snapshot().phase().active();
    let can_send = configured
        && state.history_available()
        && controller.agent_loaded()
        && !active
        && command.is_none()
        && catalog_available
        && !workflow_active
        && !draft.trim().is_empty();
    let status = if !state.history_available() {
        "Assistant history needs attention"
    } else if !(studio.live())() {
        "Reconnecting to Studio…"
    } else if !controller.agent_loaded() {
        "Connecting to Assistant…"
    } else {
        match state.session().phase() {
            WorkflowAgentPhase::Responding => "Assistant is working…",
            WorkflowAgentPhase::WaitingForRun => "Waiting for the Workflow run…",
            WorkflowAgentPhase::Cancelling => "Stopping Assistant turn…",
            WorkflowAgentPhase::Failed => "Assistant turn failed",
            WorkflowAgentPhase::Cancelled => "Assistant turn stopped",
            WorkflowAgentPhase::Interrupted => "Assistant turn was interrupted",
            WorkflowAgentPhase::Idle => {
                if configured {
                    "Ready"
                } else {
                    "Assistant unavailable"
                }
            }
        }
    };
    let messages = state.session().messages().to_owned();
    let latest_signature = messages
        .last()
        .map(|message| {
            format!(
                "{}:{}:{}:{:?}:{:?}",
                messages.len(),
                message.text().len(),
                message.tools().len(),
                message.phase(),
                state.session().phase(),
            )
        })
        .unwrap_or_else(|| format!("0:{:?}", state.session().phase()));
    use_effect(use_reactive((&latest_signature,), move |_| {
        if !*follow_latest.peek() {
            return;
        }
        let tail = thread_tail.peek().to_owned();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(24).await;
            if let Some(tail) = tail {
                let _ = tail.scroll_to(ScrollBehavior::Smooth).await;
            }
        });
    }));
    let composer_hint = AssistantComposerState {
        availability: AssistantAvailability {
            history_available: state.history_available(),
            studio_live: (studio.live())(),
            agent_loaded: controller.agent_loaded(),
            configured,
            catalog_available,
        },
        activity: AssistantActivity {
            active,
            command,
            workflow_active,
            draft_empty: draft.trim().is_empty(),
        },
    }
    .hint();
    let starter_relations_key = draft_key.clone();
    let starter_siglip_key = draft_key.clone();
    let submit_key = draft_key.clone();
    let input_key = draft_key.clone();
    let keydown_key = draft_key.clone();
    let send_key = draft_key;
    rsx! {
        section { class: "workflow-assistant", "aria-label": "Workflow assistant",
            header { class: "workflow-assistant-intro",
                div { class: "workflow-assistant-heading",
                    h2 { "Assistant" }
                    span {
                        class: if active { "workflow-assistant-presence is-active" } else if configured { "workflow-assistant-presence is-ready" } else { "workflow-assistant-presence" },
                        role: "status",
                        "aria-live": "polite",
                        "aria-atomic": "true",
                        "{status}"
                    }
                }
                div { class: "workflow-assistant-conversation-controls",
                    label { r#for: "workflow-agent-conversation", "Conversation" }
                    div { class: "workflow-assistant-conversation-row",
                        select {
                            id: "workflow-agent-conversation",
                            class: "workflow-assistant-conversation-select",
                            disabled: conversation_reason.is_some(),
                            value: "{state.active_conversation_id()}",
                            title: conversation_reason.clone().unwrap_or_else(|| "Switch Assistant conversation".to_owned()),
                            onchange: move |event| {
                                controller.activate_agent_conversation(event.value(), studio);
                            },
                            for conversation in &state.conversations() {
                                option {
                                    value: "{conversation.conversation_id()}",
                                    selected: conversation.conversation_id() == state.active_conversation_id(),
                                    "{conversation.option_label()}"
                                }
                            }
                        }
                        Action {
                            purpose: ActionPurpose::Create,
                            variant: ActionVariant::Outline,
                            class: "workflow-assistant-new-action",
                            aria_label: "New Assistant conversation",
                            disabled: new_conversation_reason.is_some(),
                            title: new_conversation_reason.clone().unwrap_or_else(|| "Start a new Assistant conversation".to_owned()),
                            onclick: move |_| controller.create_agent_conversation(studio),
                            if command == Some(AgentPendingCommand::CreatingConversation) { "Creating…" } else { "New" }
                        }
                    }
                    if let Some(reason) = conversation_reason.as_ref().or(new_conversation_reason.as_ref()) {
                        p { class: "workflow-assistant-conversation-reason", role: "status", "{reason}" }
                    }
                }
            }
            div {
                id: "workflow-assistant-scroll",
                class: "workflow-assistant-scroll",
                onscroll: move |event| {
                    let distance = f64::from(event.data().scroll_height() - event.data().client_height())
                        - event.data().scroll_top();
                    let near_latest = distance <= 64.0;
                    follow_latest.set(near_latest);
                    show_latest_action.set(!near_latest);
                },
                if messages.is_empty() {
                    section { class: "workflow-assistant-empty",
                        h3 { "What should we find?" }
                        p { "Describe a moment or comparison. I’ll inspect Spiral, build the graph, and show the evidence." }
                        if configured {
                            div { class: "workflow-assistant-examples",
                                Action {
                                    purpose: ActionPurpose::Select,
                                    variant: ActionVariant::Outline,
                                    onclick: {
                                        let draft_key = starter_relations_key.clone();
                                        move |_| {
                                            drafts.write().insert(draft_key.clone(), "Show the available Spiral relations and build a read-only Table → Query → Result workflow for the most relevant one.".to_owned());
                                        }
                                    },
                                    "Find an interesting relation and summarize it"
                                }
                                Action {
                                    purpose: ActionPurpose::Select,
                                    variant: ActionVariant::Outline,
                                    onclick: {
                                        let draft_key = starter_siglip_key.clone();
                                        move |_| {
                                            drafts.write().insert(draft_key.clone(), "Find an image relation, add SigLIP concepts for gripper and cube, run the workflow, and summarize the bounded results.".to_owned());
                                        }
                                    },
                                    "Find moments when the gripper holds an object"
                                }
                            }
                        }
                    }
                } else {
                    ol { class: "workflow-assistant-thread",
                        for message in messages.clone() {
                            li {
                                class: match message.role() { WorkflowAgentRole::User => "workflow-agent-message is-user", WorkflowAgentRole::Assistant => "workflow-agent-message is-assistant", WorkflowAgentRole::Synthetic => "workflow-agent-message is-system" },
                                div { class: "workflow-agent-message-meta",
                                    strong { match message.role() { WorkflowAgentRole::User => "You", WorkflowAgentRole::Assistant => "Assistant", WorkflowAgentRole::Synthetic => "Workflow" } }
                                    if message.phase() == WorkflowAgentMessagePhase::Streaming {
                                        span { "Working…" }
                                    }
                                }
                                if !message.text().is_empty() {
                                    {AssistantDocument::parse(message.text()).render()}
                                }
                                if !message.tools().is_empty() {
                                    details {
                                        class: "workflow-agent-tools-disclosure",
                                        open: message.phase() == WorkflowAgentMessagePhase::Failed,
                                        summary { "{message.tools().trace_summary(message.phase())}" }
                                        ul { class: "workflow-agent-tools", "aria-label": "Assistant execution trace",
                                        for tool in message.tools().to_owned() {
                                            li { class: match tool.phase() { WorkflowAgentToolPhase::Running => "is-running", WorkflowAgentToolPhase::Complete => "is-complete", WorkflowAgentToolPhase::Failed => "is-failed" },
                                                span { class: "workflow-agent-tool-mark", aria_hidden: "true" }
                                                div { class: "workflow-agent-tool-detail",
                                                    strong { "{tool.summary()}" }
                                                    if let Some(error) = tool.error() { p { "{error}" } }
                                                    if let Some(receipt) = tool.receipt() {
                                                        if controller.receipt_panel(receipt).is_some() {
                                                            {
                                                                let target = receipt.to_owned();
                                                                let label = receipt.label().to_owned();
                                                                rsx! {
                                                                    Action {
                                                                        purpose: ActionPurpose::Navigate,
                                                                        variant: ActionVariant::Link,
                                                                        class: "workflow-agent-receipt-action",
                                                                        onclick: move |_| controller.open_agent_receipt(target.clone()),
                                                                        "{label}"
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(error) = controller
                    .agent_error()
                    .or_else(|| state.session().error().map(str::to_owned))
                {
                    div { class: "workflow-assistant-error", role: "alert",
                        strong { "Assistant needs attention" }
                        p { "{error}" }
                        if state.history_available() && !active && configured {
                            Action {
                                purpose: ActionPurpose::Retry,
                                variant: ActionVariant::Outline,
                                disabled: command.is_some(),
                                onclick: move |_| controller.retry_agent(studio),
                                "Retry last turn"
                            }
                        }
                    }
                }
                div {
                    class: "workflow-assistant-thread-tail",
                    "aria-hidden": "true",
                    onmounted: move |event| thread_tail.set(Some(event.data())),
                }
            }
            form {
                class: "workflow-assistant-composer",
                onsubmit: move |event| {
                    event.prevent_default();
                    if can_send {
                        let text = drafts.peek().get(&submit_key).cloned().unwrap_or_default();
                        controller.send_agent(text, studio);
                    }
                },
                label { r#for: "workflow-agent-input", "Message the Workflow Assistant" }
                if show_latest_action() {
                    Action {
                        purpose: ActionPurpose::Navigate,
                        variant: ActionVariant::Outline,
                        class: "workflow-assistant-latest-action",
                        onclick: move |_| {
                            follow_latest.set(true);
                            show_latest_action.set(false);
                            let tail = thread_tail.peek().to_owned();
                            spawn(async move {
                                if let Some(tail) = tail {
                                    let _ = tail.scroll_to(ScrollBehavior::Smooth).await;
                                }
                            });
                        },
                        "Jump to latest"
                    }
                }
                textarea {
                    id: "workflow-agent-input",
                    rows: "1",
                    maxlength: "8000",
                    disabled: !state.history_available() || !configured || active || command.is_some() || !catalog_available,
                    placeholder: if !state.history_available() { "Assistant history is unavailable" } else if configured { "Ask about your robotics data…" } else { "Assistant is temporarily unavailable" },
                    value: "{draft}",
                    oninput: {
                        let draft_key = input_key.clone();
                        move |event| {
                            drafts.write().insert(draft_key.clone(), event.value());
                        }
                    },
                    onkeydown: move |event| {
                        if event.key() == Key::Enter
                            && !event.modifiers().shift()
                            && can_send
                        {
                            event.prevent_default();
                            let text = drafts.peek().get(&keydown_key).cloned().unwrap_or_default();
                            controller.send_agent(text, studio);
                        }
                    },
                }
                div { class: "workflow-assistant-composer-actions",
                    span { role: "status", "{composer_hint}" }
                    if active {
                        Action {
                            purpose: ActionPurpose::Stop,
                            variant: ActionVariant::Outline,
                            disabled: command.is_some(),
                            aria_busy: command == Some(AgentPendingCommand::Cancelling),
                            onclick: move |_| controller.cancel_agent(studio),
                            if command == Some(AgentPendingCommand::Cancelling) { "Stopping…" } else { "Stop" }
                        }
                    } else {
                        Action {
                            purpose: ActionPurpose::Start,
                            variant: ActionVariant::Solid,
                            class: "workflow-assistant-send-action",
                            disabled: !can_send,
                            aria_busy: command == Some(AgentPendingCommand::Sending),
                            title: if !catalog_available { "Reconnect Spiral before sending a message" } else if !configured { "Assistant is temporarily unavailable" } else if workflow_active { "Wait for the active Workflow run to finish before sending a message" } else { "Send this message to the Workflow Assistant" },
                            onclick: move |_| {
                                if can_send {
                                    let text = drafts.peek().get(&send_key).cloned().unwrap_or_default();
                                    controller.send_agent(text, studio);
                                }
                            },
                            if command == Some(AgentPendingCommand::Sending) { "Sending…" } else { "Send" }
                        }
                    }
                }
            }
        }
    }
}

impl AcceptedAgentDraft {
    fn clear_from(&self, drafts: &mut BTreeMap<AgentDraftKey, String>) -> bool {
        let key = AgentDraftKey::new(self.scope_id(), self.conversation_id());
        if drafts.get(&key).map(String::as_str) != Some(self.text()) {
            return false;
        }
        drafts.remove(&key);
        true
    }
}

trait ConversationPresentation {
    fn option_label(&self) -> String;
}

impl ConversationPresentation for WorkflowAgentConversationSummary {
    fn option_label(&self) -> String {
        if self.message_count() == 0 {
            return self.title().to_owned();
        }
        let noun = if self.message_count() == 1 {
            "message"
        } else {
            "messages"
        };
        format!("{} · {} {noun}", self.title(), self.message_count())
    }
}

trait ToolTracePresentation {
    fn trace_summary(&self, message_phase: WorkflowAgentMessagePhase) -> String;
}

impl ToolTracePresentation for [WorkflowAgentToolSummary] {
    fn trace_summary(&self, message_phase: WorkflowAgentMessagePhase) -> String {
        if let Some(tool) = self
            .iter()
            .rev()
            .find(|tool| tool.phase() == WorkflowAgentToolPhase::Running)
        {
            return format!("Working · {}", tool.summary());
        }
        let failed = self
            .iter()
            .filter(|tool| tool.phase() == WorkflowAgentToolPhase::Failed)
            .count();
        if failed > 0 {
            let label = match message_phase {
                WorkflowAgentMessagePhase::Complete => "recovered",
                WorkflowAgentMessagePhase::Streaming => "recovering",
                WorkflowAgentMessagePhase::Failed | WorkflowAgentMessagePhase::Cancelled => {
                    "failed"
                }
            };
            let noun = if self.len() == 1 { "step" } else { "steps" };
            return format!("{} {noun} · {failed} {label}", self.len());
        }
        let noun = if self.len() == 1 { "step" } else { "steps" };
        format!("{} {noun} completed", self.len())
    }
}

struct AssistantAvailability {
    history_available: bool,
    studio_live: bool,
    agent_loaded: bool,
    configured: bool,
    catalog_available: bool,
}

struct AssistantActivity {
    active: bool,
    command: Option<AgentPendingCommand>,
    workflow_active: bool,
    draft_empty: bool,
}

struct AssistantComposerState {
    availability: AssistantAvailability,
    activity: AssistantActivity,
}

impl AssistantComposerState {
    fn hint(&self) -> &'static str {
        if !self.availability.history_available {
            return "Assistant history needs attention.";
        }
        if !self.availability.studio_live {
            return "Reconnecting to Studio…";
        }
        if !self.availability.agent_loaded {
            return "Connecting to Assistant…";
        }
        if !self.availability.catalog_available {
            return "Connect to Spiral to ask about this project.";
        }
        if !self.availability.configured {
            return "Assistant is temporarily unavailable. Retry in a moment.";
        }
        if self.activity.active {
            return "Assistant is working. You can stop this turn.";
        }
        if self.activity.command.is_some() {
            return "Finishing the current Assistant action…";
        }
        if self.activity.workflow_active {
            return "Waiting for the active Workflow run to finish.";
        }
        if self.activity.draft_empty {
            return "Enter to send · Shift+Enter for a new line.";
        }
        "Enter to send · Shift+Enter for a new line."
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssistantDocument(Vec<AssistantBlock>);

impl AssistantDocument {
    fn parse(text: &str) -> Self {
        let lines = text.lines().collect::<Vec<_>>();
        let mut document = Self(Vec::new());
        let mut paragraph = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index];
            if let Some(language) = line.trim_start().strip_prefix("```") {
                document.flush_paragraph(&mut paragraph);
                let language = language.bounded_markdown_language();
                index += 1;
                let mut code = Vec::new();
                while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                    code.push(lines[index]);
                    index += 1;
                }
                if index < lines.len() {
                    index += 1;
                }
                document.0.push(AssistantBlock::Code {
                    language,
                    text: code.join("\n"),
                });
                continue;
            }
            if let Some(item) = line.assistant_list_item() {
                document.flush_paragraph(&mut paragraph);
                let mut items = vec![AssistantInline::parse(item)];
                index += 1;
                while index < lines.len() {
                    let Some(item) = lines[index].assistant_list_item() else {
                        break;
                    };
                    items.push(AssistantInline::parse(item));
                    index += 1;
                }
                document.0.push(AssistantBlock::List(items));
                continue;
            }
            if line.trim().is_empty() {
                document.flush_paragraph(&mut paragraph);
            } else {
                paragraph.push(line.trim());
            }
            index += 1;
        }
        document.flush_paragraph(&mut paragraph);
        document
    }

    fn flush_paragraph(&mut self, lines: &mut Vec<&str>) {
        if lines.is_empty() {
            return;
        }
        self.0
            .push(AssistantBlock::Paragraph(AssistantInline::parse(
                &lines.join(" "),
            )));
        lines.clear();
    }

    fn render(self) -> Element {
        rsx! {
            div { class: "workflow-agent-message-body",
                for (index, block) in self.0.into_iter().enumerate() {
                    {block.render(index)}
                }
            }
        }
    }

    #[allow(dead_code)]
    fn into_blocks(self) -> Vec<AssistantBlock> {
        self.0
    }
}

impl AssistantBlock {
    fn render(self, index: usize) -> Element {
        match self {
            Self::Paragraph(inlines) => rsx! {
                p { key: "paragraph-{index}", {AssistantInline::render_all(inlines)} }
            },
            Self::List(items) => rsx! {
                ul { key: "list-{index}",
                    for (item_index, item) in items.into_iter().enumerate() {
                        li { key: "item-{item_index}", {AssistantInline::render_all(item)} }
                    }
                }
            },
            Self::Code { language, text } => rsx! {
                pre { key: "code-{index}",
                    if let Some(language) = language {
                        span { class: "workflow-agent-code-language", "{language}" }
                    }
                    code { "{text}" }
                }
            },
        }
    }
}

impl AssistantInline {
    fn parse(source: &str) -> Vec<Self> {
        let mut output = Vec::new();
        let mut remaining = source;
        while !remaining.is_empty() {
            if let Some(strong) = remaining.strip_prefix("**")
                && let Some(end) = strong.find("**")
            {
                Self::push_merged(&mut output, Self::Strong(strong[..end].to_owned()));
                remaining = &strong[end + 2..];
                continue;
            }
            if let Some(code) = remaining.strip_prefix('`')
                && let Some(end) = code.find('`')
            {
                Self::push_merged(&mut output, Self::Code(code[..end].to_owned()));
                remaining = &code[end + 1..];
                continue;
            }
            let next = [remaining.find("**"), remaining.find('`')]
                .into_iter()
                .flatten()
                .filter(|index| *index > 0)
                .min()
                .unwrap_or(remaining.len());
            if next == remaining.len() {
                Self::push_merged(&mut output, Self::Text(remaining.to_owned()));
                break;
            }
            Self::push_merged(&mut output, Self::Text(remaining[..next].to_owned()));
            remaining = &remaining[next..];
            if remaining.starts_with("**") && !remaining[2..].contains("**") {
                Self::push_merged(&mut output, Self::Text("**".to_owned()));
                remaining = &remaining[2..];
            } else if remaining.starts_with('`') && !remaining[1..].contains('`') {
                Self::push_merged(&mut output, Self::Text("`".to_owned()));
                remaining = &remaining[1..];
            }
        }
        output
    }

    fn push_merged(output: &mut Vec<Self>, inline: Self) {
        match (output.last_mut(), inline) {
            (Some(Self::Text(existing)), Self::Text(next)) => existing.push_str(&next),
            (_, inline) => output.push(inline),
        }
    }

    fn render_all(inlines: Vec<Self>) -> Element {
        rsx! {
            for (index, inline) in inlines.into_iter().enumerate() {
                match inline {
                    Self::Text(text) => rsx! { span { key: "text-{index}", "{text}" } },
                    Self::Strong(text) => rsx! { strong { key: "strong-{index}", "{text}" } },
                    Self::Code(text) => rsx! { code { key: "code-{index}", "{text}" } },
                }
            }
        }
    }
}

trait AssistantMarkdownLine {
    fn assistant_list_item(&self) -> Option<&str>;
    fn bounded_markdown_language(&self) -> Option<String>;
}

impl AssistantMarkdownLine for str {
    fn assistant_list_item(&self) -> Option<&str> {
        let line = self.trim_start();
        line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
    }

    fn bounded_markdown_language(&self) -> Option<String> {
        let language = self
            .trim()
            .chars()
            .take(24)
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .collect::<String>();
        (!language.is_empty()).then_some(language)
    }
}

#[component]
fn WorkflowNodeInspector() -> Element {
    let controller = workflow_controller();
    let selected = controller.selected_node_data();
    let relations = controller.relations();
    let snapshot_active = controller.snapshot().phase().active();
    let command_pending = controller.command().is_some();
    let active = snapshot_active || command_pending;
    rsx! {
        section { class: "workflow-inspector", "aria-label": "Selected workflow node",
            if let Some((node_id, title, data)) = selected {
                header { class: "workflow-inspector-heading",
                    div { class: "workflow-inspector-kind",
                        {node_kind_icon(data.kind())}
                        span { "{data.kind_label()}" }
                    }
                    h2 { "{title}" }
                    p { "{data.description()}" }
                }
                if let Some(error) = &data.error() {
                    div { class: "workflow-inspector-error", role: "alert",
                        strong { "{data.phase_label()}" }
                        p { "{error}" }
                    }
                }
                if !data.warnings().is_empty() {
                    div { class: "workflow-inspector-warning", role: "status",
                        strong { "Completed with warnings" }
                        for warning in &data.warnings() { p { "{warning}" } }
                    }
                }
                dl { class: "workflow-inspector-facts",
                    div {
                        dt { "State" }
                        dd { class: "{data.phase_class()}", "{data.phase_label()} · {data.detail()}" }
                    }
                    if let Some(relation) = &data.relation() {
                        div {
                            dt { "Relation" }
                            dd { code { "{relation}" } }
                        }
                    }
                    if let Some(output) = data
                        .feature_cache_relation()
                        .filter(|output| !output.is_empty())
                    {
                        div {
                            dt { "Writes" }
                            dd { code { "{output}" } }
                        }
                    }
                    if !data.fields().is_empty() {
                        div {
                            dt { "Fields" }
                            dd { class: "workflow-inspector-fields",
                                for field in &data.fields() { code { "{field}" } }
                            }
                        }
                    }
                    if let Some(progress) = &data.progress() {
                        div {
                            dt { "Progress" }
                            dd { class: "font-mono tabular-nums", "{progress.completed()} / {progress.total()} {progress.unit()}" }
                        }
                    }
                    if data.quarantine().count() > 0 {
                        div {
                            dt { "Quarantine" }
                            dd { "{data.quarantine().count()} rows" }
                        }
                    }
                    if let Some(model) = &data.model_run() {
                        div {
                            dt { "Device" }
                            dd { "{model.device()}" }
                        }
                        div {
                            dt { "Materialization" }
                            dd {
                                if model.reused() { "Reused · " } else { "Computed · " }
                                span { class: "font-mono tabular-nums", "{model.decode_ms()} ms decode · {model.inference_ms()} ms inference" }
                            }
                        }
                        div {
                            dt { "Fingerprint" }
                            dd { code { title: "{model.fingerprint()}", "{model.fingerprint().chars().take(12).collect::<String>()}" } }
                        }
                    }
                }
                if data.kind() == WorkflowNodeKind::Table {
                    section { class: "workflow-inspector-editor",
                        label {
                            span { class: "label", "Spiral relation" }
                            select {
                                class: "field",
                                disabled: active,
                                value: data.relation().unwrap_or_default(),
                                onchange: {
                                    let node_id = node_id.clone();
                                    move |event| controller.set_table_relation(&node_id, event.value())
                                },
                                option { value: "", "Choose a qualified relation…" }
                                for relation in &relations {
                                    option {
                                        value: "{relation.name()}",
                                        selected: data.relation() == Some(relation.name()),
                                        "{relation.name()}"
                                    }
                                }
                            }
                        }
                        if relations.is_empty() {
                            p { class: "workflow-inspector-help", role: "status",
                                "No catalog relations are available. Configure an active Spiral project or materialize data in Data first."
                            }
                        }
                        if let Some(limitation) = controller.relation_limitation() {
                            p { class: "workflow-inspector-help", "{limitation}" }
                        }
                    }
                    p { class: "workflow-inspector-boundary",
                        "Import provenance belongs in Data. Workflow always begins from qualified Spiral relations."
                    }
                } else if data.kind() == WorkflowNodeKind::Query {
                    section { class: "workflow-inspector-editor workflow-inspector-query-editor",
                        label {
                            span { class: "label", "Read-only SQL" }
                            textarea {
                                class: "query-input workflow-query-input",
                                spellcheck: "false",
                                disabled: active,
                                value: data.sql().unwrap_or_default(),
                                oninput: {
                                    let node_id = node_id.clone();
                                    move |event| controller.set_query_sql(&node_id, event.value())
                                },
                            }
                        }
                        p { class: "workflow-inspector-help",
                            "Reference each connected Table, candidate Query, or Model feature cache with an exact placeholder such as "
                            code { "{{node:table}}" }
                            "."
                        }
                    }
                } else if data.kind() == WorkflowNodeKind::Model {
                    section { class: "workflow-inspector-editor workflow-inspector-model-editor",
                        label {
                            span { class: "label", "Model" }
                            select {
                                class: "field",
                                disabled: true,
                                value: "siglip-base",
                                option { value: "siglip-base", "SigLIP base" }
                            }
                        }
                        p { class: "workflow-inspector-help",
                            "The candidate Query must project the five stable frame identity fields and each decoded spql.image.Image field named below. Studio registers compatible definitions and fills only missing applicable cells in the shared source cache; decoded images are never cached."
                        }
                        div { class: "workflow-feature-definitions",
                            for (index, feature) in data.features().iter().enumerate() {
                                section { class: "workflow-feature-definition",
                                    header {
                                        strong { "Feature {index + 1}" }
                                        if data.features().len() > 1 {
                                            Action {
                                                purpose: ActionPurpose::Remove,
                                                disabled: active,
                                                title: "Remove this feature definition",
                                                onclick: {
                                                    let node_id = node_id.clone();
                                                    move |_| controller.remove_model_feature(&node_id, index)
                                                },
                                                "Remove"
                                            }
                                        }
                                    }
                                    label {
                                        span { class: "label", "Feature name" }
                                        input {
                                            class: "field font-mono",
                                            r#type: "text",
                                            spellcheck: "false",
                                            disabled: active,
                                            placeholder: "is_gripper_open_feature",
                                            value: feature.name().to_owned(),
                                            oninput: {
                                                let node_id = node_id.clone();
                                                move |event| {
                                                    let value = event.value();
                                                    controller.set_model_feature(ModelFeatureTarget::new(&node_id, index), move |feature| feature.set_name(value));
                                                }
                                            },
                                        }
                                    }
                                    label {
                                        span { class: "label", "Description" }
                                        input {
                                            class: "field",
                                            r#type: "text",
                                            disabled: active,
                                            placeholder: "Probability that the gripper is open",
                                            value: feature.description().to_owned(),
                                            oninput: {
                                                let node_id = node_id.clone();
                                                move |event| {
                                                    let value = event.value();
                                                    controller.set_model_feature(ModelFeatureTarget::new(&node_id, index), move |feature| feature.set_description(value));
                                                }
                                            },
                                        }
                                    }
                                    label {
                                        span { class: "label", "Model concept" }
                                        input {
                                            class: "field",
                                            r#type: "text",
                                            disabled: active,
                                            placeholder: "gripper open",
                                            value: feature.concept().to_owned(),
                                            oninput: {
                                                let node_id = node_id.clone();
                                                move |event| {
                                                    let value = event.value();
                                                    controller.set_model_feature(ModelFeatureTarget::new(&node_id, index), move |feature| feature.set_concept(value));
                                                }
                                            },
                                        }
                                    }
                                    div { class: "workflow-feature-applicability",
                                        label {
                                            span { class: "label", "Candidate image field" }
                                            input {
                                                class: "field font-mono",
                                                r#type: "text",
                                                spellcheck: "false",
                                                disabled: active,
                                                value: feature.applicability().media_field().to_owned(),
                                                oninput: {
                                                    let node_id = node_id.clone();
                                                    move |event| {
                                                        let value = event.value();
                                                        controller.set_model_feature(ModelFeatureTarget::new(&node_id, index), move |feature| feature.set_media_field(value));
                                                    }
                                                },
                                            }
                                        }
                                        label {
                                            span { class: "label", "Streams (optional)" }
                                            input {
                                                class: "field",
                                                r#type: "text",
                                                disabled: active,
                                                placeholder: "camera.top, camera.wrist",
                                                value: feature.applicability().stream_ids().join(", "),
                                                oninput: {
                                                    let node_id = node_id.clone();
                                                    move |event| {
                                                        let streams = event.value().split(',').map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned).collect();
                                                        controller.set_model_feature(ModelFeatureTarget::new(&node_id, index), move |feature| feature.set_stream_ids(streams));
                                                    }
                                                },
                                            }
                                        }
                                    }
                                    p { class: "workflow-inspector-help",
                                        "Source "
                                        if feature.applicability().source_relation().is_empty() {
                                            span { "Not configured" }
                                        } else {
                                            code { "{feature.applicability().source_relation()}" }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "workflow-inspector-actions",
                            Action {
                                purpose: ActionPurpose::Create,
                                variant: ActionVariant::Outline,
                                disabled: active,
                                title: "Add another registered feature definition",
                                onclick: {
                                    let node_id = node_id.clone();
                                    move |_| controller.add_model_feature(&node_id)
                                },
                                "Add feature"
                            }
                            Action {
                                purpose: ActionPurpose::Remove,
                                variant: ActionVariant::Outline,
                                disabled: active,
                                title: if snapshot_active { "Cancel the active run before removing this Model node" } else if command_pending { "Wait for the current Workflow command before removing this Model node" } else { "Remove this Model and its generated feature join, then restore the candidate Query as the terminal read" },
                                onclick: {
                                    let node_id = node_id.clone();
                                    move |_| controller.remove_siglip_model(&node_id)
                                },
                                "Remove model"
                            }
                        }
                    }
                    if !data.quarantine().examples().is_empty() {
                        section { class: "workflow-inspector-boundary workflow-quarantine-examples",
                            strong { "Quarantine examples" }
                            for example in &data.quarantine().examples() { code { "{example}" } }
                        }
                    }
                }
                {node_validation_issues(&node_id)}
            } else {
                div { class: "workflow-panel-empty",
                    strong { "No node selected" }
                    p { "Choose a node on the canvas to inspect or edit its relation contract." }
                    span { "Keyboard: Tab, then Enter or Space" }
                }
            }
        }
    }
}

fn node_validation_issues(node_id: &str) -> Element {
    let validation = workflow_controller().validation();
    let issues = validation
        .errors()
        .iter()
        .filter(|issue| issue.node_id().is_none_or(|id| id == node_id))
        .collect::<Vec<_>>();
    rsx! {
        if !issues.is_empty() {
            section { class: "workflow-validation", role: "alert",
                for issue in issues {
                    div {
                        strong { "{issue.message()}" }
                        p { "{issue.recovery()}" }
                    }
                }
            }
        }
    }
}

#[component]
fn WorkflowCanvas() -> Element {
    let controller = workflow_controller();
    let studio = studio();
    let nodes = controller.nodes();
    let edges = controller.edges();
    let flow = use_flow_handle::<WorkflowNodeData>();
    let mut canvas_was_active = use_signal(|| false);
    let initial_topology = controller.topology_signature();
    let observed_initial_topology = initial_topology.clone();
    let mut observed_topology = use_signal(move || observed_initial_topology);
    // A persisted graph can mount with old/default positions even when its
    // topology is already authoritative. Treat first visibility as pending a
    // compact layout; later activations fit only when the topology is stable.
    let mut canvas_topology = use_signal(String::new);
    let mut canvas_resize_revision = use_signal(|| 0u64);
    let spec = controller.spec();
    let snapshot = controller.snapshot();
    let run_reason = controller.run_disabled_reason();
    let apply_reason = controller.apply_disabled_reason();
    let command = controller.command();
    let active = snapshot.phase().active();
    let topology_blocked = active || command.is_some();
    let agent_topology_blocked =
        controller.agent_state().session().phase().active() || controller.agent_command().is_some();
    let state_load_failed = controller.load_error().is_some();
    let dirty = controller.dirty();
    let empty = spec.nodes().is_empty() && spec.edges().is_empty();
    let edit_label = if dirty {
        "Modified".to_owned()
    } else {
        format!("Saved · r{}", spec.revision())
    };
    let complete_with_warnings = snapshot.phase() == WorkflowRunPhase::Completed
        && snapshot.nodes().iter().any(|node| {
            node.phase() == WorkflowNodePhase::CompleteWithWarnings || !node.warnings().is_empty()
        });
    let run_label = if complete_with_warnings {
        "Complete with warnings"
    } else {
        snapshot.phase().run_label()
    };

    use_effect(move || {
        let selected = nodes
            .read()
            .iter()
            .find(|node| node.selected)
            .map(|node| node.id.clone());
        match (selected, controller.selected_node()) {
            (Some(next), current) if current.as_deref() != Some(next.as_str()) => {
                controller.select_node(next);
            }
            (None, Some(_)) => controller.clear_selection(),
            _ => {}
        }
    });

    use_effect(move || {
        let revision = canvas_resize_revision();
        if revision == 0 {
            return;
        }
        spawn(async move {
            // Panel and window resizes can arrive in a burst. Fit once after
            // the final measured size so a narrower canvas never clips the
            // terminal nodes off-screen.
            gloo_timers::future::TimeoutFuture::new(80).await;
            if *canvas_resize_revision.peek() == revision {
                flow.fit_workflow_view(160);
            }
        });
    });

    use_effect(move || {
        let topology = controller.topology_signature();
        if topology == *observed_topology.peek() {
            return;
        }
        observed_topology.set(topology);
        spawn(async move {
            // Give a new card one rendered frame so dioxus-flow includes its
            // measured bounds in layered layout and the accompanying fit.
            gloo_timers::future::TimeoutFuture::new(48).await;
            flow.auto_layout(&WorkflowCanvasLayout::options());
        });
    });

    use_effect(move || {
        let canvas_active = controller
            .active_panel()
            .as_ref()
            .is_some_and(|panel| panel.as_str() == CANVAS_PANEL_ID);
        let was_active = *canvas_was_active.peek();
        if canvas_active != was_active {
            canvas_was_active.set(canvas_active);
        }
        if canvas_active && !was_active {
            let topology = controller.topology_signature();
            let needs_layout = topology != *canvas_topology.peek();
            canvas_topology.set(topology);
            spawn(async move {
                // Wait one frame so dioxus-flow measures the newly active
                // canvas rather than its former hidden bounds.
                gloo_timers::future::TimeoutFuture::new(48).await;
                if needs_layout {
                    flow.auto_layout(&WorkflowCanvasLayout::options());
                } else {
                    flow.fit_view(300);
                }
            });
        }
    });

    rsx! {
        section { class: "workflow-canvas", "aria-label": "Spiral relation workflow",
            header { class: "workflow-canvas-toolbar",
                div {
                    strong { class: if dirty { "workflow-edit-badge is-modified" } else { "workflow-edit-badge" }, "{edit_label}" }
                    strong { class: snapshot.phase().badge_class(complete_with_warnings), "{run_label}" }
                    span { "{spec.nodes().len()} nodes · {spec.edges().len()} connections" }
                }
                if !spec.nodes().is_empty() {
                    Action {
                        purpose: ActionPurpose::Create,
                        variant: ActionVariant::Outline,
                        disabled: topology_blocked || agent_topology_blocked,
                        title: if active { "Cancel the active run before starting a new Workflow" } else if command.is_some() { "Wait for the current Workflow command before starting a new Workflow" } else if agent_topology_blocked { "Stop the active Assistant turn before starting a new Workflow" } else { "Clear the canvas as an unapplied draft; Apply replaces the saved Workflow" },
                        onclick: move |_| controller.clear_workflow_draft(),
                        "New workflow"
                    }
                }
                if !spec.nodes().is_empty() && !controller.has_model() {
                    Action {
                        purpose: ActionPurpose::Create,
                        variant: ActionVariant::Outline,
                        disabled: topology_blocked,
                        title: if snapshot.phase().active() { "Cancel the active run before adding a Model node" } else if command.is_some() { "Wait for the current Workflow command before adding a Model node" } else { "Insert SigLIP between the first Table and Query" },
                        onclick: move |_| controller.add_siglip_model(),
                        "Add model"
                    }
                }
                Action {
                    purpose: ActionPurpose::Fit,
                    variant: ActionVariant::Outline,
                    class: "workflow-fit-action",
                    disabled: empty,
                    title: if empty { "The workflow canvas is empty" } else { "Fit the workflow graph in the canvas" },
                    onclick: move |_| flow.fit_workflow_view(300),
                    FitViewIcon {}
                    "Fit"
                }
                if controller.revision_conflict() {
                    Action {
                        purpose: ActionPurpose::Sync,
                        variant: ActionVariant::Outline,
                        title: "Discard local edits and reload the newer saved Workflow",
                        onclick: move |_| controller.reload_saved(),
                        "Reload saved"
                    }
                } else if dirty {
                    Action {
                        purpose: ActionPurpose::Sync,
                        variant: ActionVariant::Outline,
                        title: "Discard unapplied graph changes and restore the saved Workflow",
                        onclick: move |_| controller.reload_saved(),
                        "Discard changes"
                    }
                }
                Action {
                    purpose: ActionPurpose::Apply,
                    variant: ActionVariant::Outline,
                    disabled: apply_reason.is_some(),
                    aria_busy: command == Some(WorkflowPendingCommand::Applying),
                    title: apply_reason.clone().unwrap_or_else(|| "Apply graph changes".to_owned()),
                    onclick: move |_| controller.apply(studio),
                    if command == Some(WorkflowPendingCommand::Applying) { "Applying…" } else { "Apply" }
                }
                if active {
                    Action {
                        purpose: ActionPurpose::Cancel,
                        variant: ActionVariant::Outline,
                        disabled: command.is_some() || snapshot.phase() == WorkflowRunPhase::Cancelling,
                        aria_busy: command == Some(WorkflowPendingCommand::Cancelling),
                        title: "Cancel after the current Spiral operation",
                        onclick: move |_| controller.cancel(studio),
                        if snapshot.phase() == WorkflowRunPhase::Cancelling || command == Some(WorkflowPendingCommand::Cancelling) { "Cancelling…" } else { "Cancel" }
                    }
                } else {
                    Action {
                        purpose: ActionPurpose::Start,
                        variant: ActionVariant::Solid,
                        disabled: run_reason.is_some(),
                        aria_busy: command == Some(WorkflowPendingCommand::Starting),
                        title: run_reason.clone().unwrap_or_else(|| "Run this saved workflow".to_owned()),
                        onclick: move |_| controller.run(studio),
                        if command == Some(WorkflowPendingCommand::Starting) { "Starting…" } else { "Run workflow" }
                    }
                }
            }
            if let Some(reason) = run_reason {
                p {
                    class: if state_load_failed { "workflow-canvas-status workflow-canvas-status-error" } else { "workflow-canvas-status" },
                    role: if state_load_failed { "alert" } else { "status" },
                    "{reason}"
                }
            }
            div {
                class: "workflow-flow-wrap",
                onresize: move |_| {
                    let next = canvas_resize_revision.peek().wrapping_add(1);
                    canvas_resize_revision.set(next);
                },
                Flow {
                    id: "workflow-relation-graph",
                    class: "workflow-flow",
                    nodes,
                    edges,
                    handle: flow,
                    fit_view: true,
                    fit_view_padding: 0.10,
                    min_zoom: WORKFLOW_MIN_ZOOM,
                    max_zoom: WORKFLOW_MAX_ZOOM,
                    drag_threshold: 4.0,
                    delete_key: false,
                    is_valid_connection: move |_| false,
                    on_node_click: move |id: String| controller.select_node(id),
                    on_pane_click: move |_| controller.clear_selection(),
                    node_view: move |ctx: NodeViewCtx<WorkflowNodeData>| rsx! {
                        WorkflowNodeCard { ctx }
                    },
                    Background { variant: BackgroundVariant::Dots, gap: 20.0, size: 0.7 }
                }
                if empty {
                    div { class: "workflow-flow-empty", role: "status", "aria-live": "polite",
                        WorkflowIcon {}
                        strong { "Empty workflow" }
                        p {
                            if dirty {
                                "Apply to start fresh. Then ask the Assistant to build from your Spiral relations."
                            } else {
                                "Ask the Assistant a question. Its saved nodes and connections will appear here."
                            }
                        }
                    }
                }
            }
        }
    }
}

struct WorkflowCanvasLayout;

impl WorkflowCanvasLayout {
    fn options() -> LayoutOptions {
        LayoutOptions::default()
            .direction(LayoutDirection::LeftToRight)
            // Smooth-step edges reserve a 20 px stub at both ends. Keep the rank
            // gap safely above 40 px so those stubs never overlap and detour into
            // the visibly twisted paths seen in the prototype.
            .gaps(32.0, 56.0)
    }
}

trait WorkflowFlowPresentation {
    fn fit_workflow_view(self, duration_ms: u64);
}

impl WorkflowFlowPresentation for FlowHandle<WorkflowNodeData> {
    fn fit_workflow_view(self, duration_ms: u64) {
        self.fit_view(duration_ms);
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(duration_ms.saturating_add(16) as u32).await;
            let Some(viewport) = self.viewport() else {
                return;
            };
            if viewport.zoom >= WORKFLOW_MIN_ZOOM {
                return;
            }
            let Some(core) = self.core() else {
                return;
            };
            let rect = *core.container.peek();
            let center = Point::new(rect.width / 2.0, rect.height / 2.0);
            self.set_viewport(
                viewport.zoom_about(
                    WORKFLOW_MIN_ZOOM,
                    center,
                    WORKFLOW_MIN_ZOOM,
                    WORKFLOW_MAX_ZOOM,
                ),
                0,
            );
        });
    }
}

#[component]
fn WorkflowNodeCard(ctx: NodeViewCtx<WorkflowNodeData>) -> Element {
    let node = &ctx.node;
    let data = &node.data;
    let fields = if data.fields().is_empty() {
        match data.kind() {
            WorkflowNodeKind::Table => "Choose a relation".to_owned(),
            WorkflowNodeKind::Model => "Define a feature".to_owned(),
            WorkflowNodeKind::Query => "Read-only SQL".to_owned(),
            WorkflowNodeKind::Result => "Terminal payload".to_owned(),
        }
    } else {
        data.fields().join(" · ")
    };
    let selected_class = if node.selected {
        "workflow-node-selected"
    } else {
        ""
    };
    let dragging_class = if ctx.dragging {
        "workflow-node-dragging"
    } else {
        ""
    };

    rsx! {
        article {
            class: "workflow-node-card {data.kind_class()} {data.phase_class()} {selected_class} {dragging_class}",
            title: data.error().unwrap_or_default(),
            "aria-current": if node.selected { "true" } else { "false" },
            if data.kind() != WorkflowNodeKind::Table {
                Handle { kind: HandleKind::Target, position: node.target_side }
            }
            header { class: "workflow-node-heading",
                {node_kind_icon(data.kind())}
                span { "{data.kind_label()}" }
            }
            div { class: "workflow-node-body",
                h3 { title: "{node.label}", "{node.label}" }
                p { class: "workflow-node-fields", title: "{fields}", "{fields}" }
                if let Some(relation) = &data.relation() {
                    p { class: "workflow-node-relation", "reads ", code { "{relation}" } }
                }
                if let Some(output) = data
                    .feature_cache_relation()
                    .filter(|output| !output.is_empty())
                {
                    p { class: "workflow-node-relation", "writes ", code { "{output}" } }
                }
                if let Some(sql) = &data.sql() {
                    code { class: "workflow-node-sql", "{sql}" }
                }
            }
            footer { class: "workflow-node-footer",
                span { class: "workflow-node-status-dot {data.phase_class()}", "aria-hidden": "true" }
                if let Some(progress) = &data.progress() {
                    span { "{data.phase_label()} · {progress.completed()}/{progress.total()} {progress.unit()}" }
                } else {
                    span { "{data.phase_label()} · {data.detail()}" }
                }
            }
            if let Some(progress) = &data.progress() {
                progress {
                    class: "workflow-node-progress",
                    value: "{progress.completed()}",
                    max: "{progress.total().max(1)}",
                    "aria-label": "{data.phase_label()}: {progress.completed()} of {progress.total()} {progress.unit()}"
                }
            }
            if data.kind() != WorkflowNodeKind::Result {
                Handle { kind: HandleKind::Source, position: node.source_side }
            }
        }
    }
}

fn node_kind_icon(kind: WorkflowNodeKind) -> Element {
    match kind {
        WorkflowNodeKind::Table => rsx! { TableNodeIcon {} },
        WorkflowNodeKind::Model => rsx! { ModelNodeIcon {} },
        WorkflowNodeKind::Query => rsx! { QueryNodeIcon {} },
        WorkflowNodeKind::Result => rsx! { ResultNodeIcon {} },
    }
}

#[component]
fn WorkflowResults() -> Element {
    let controller = workflow_controller();
    let studio = studio();
    let snapshot = controller.snapshot();
    let run_reason = controller.run_disabled_reason();
    let can_run_again = matches!(
        snapshot.phase(),
        WorkflowRunPhase::Cancelled | WorkflowRunPhase::Interrupted | WorkflowRunPhase::Failed
    ) && run_reason.is_none();
    let spec = controller.spec();
    let empty_workflow = spec.nodes().is_empty() && spec.edges().is_empty();
    let query = spec.nodes().iter().find_map(|node| {
        let WorkflowNodeConfig::Query { sql, max_rows } = node.config() else {
            return None;
        };
        Some((sql.to_owned(), *max_rows))
    });
    let (sql, max_rows) = query.unwrap_or_else(|| ("Read-only Workflow query".to_owned(), 100));
    let result_model = snapshot.result().cloned().map(|result| {
        QueryExecutionPresentation::new(sql.clone(), max_rows, QueryState::Ready(result))
    });
    let transient_model = if result_model.is_none() {
        match snapshot.phase() {
            WorkflowRunPhase::Validating
            | WorkflowRunPhase::Running
            | WorkflowRunPhase::Cancelling => Some(QueryExecutionPresentation::new(
                sql.clone(),
                max_rows,
                QueryState::Running,
            )),
            WorkflowRunPhase::Failed | WorkflowRunPhase::Interrupted => {
                snapshot.error().map(str::to_owned).map(|error| {
                    QueryExecutionPresentation::new(
                        sql.clone(),
                        max_rows,
                        QueryState::Failed(error),
                    )
                })
            }
            _ => None,
        }
    } else {
        None
    };
    let (empty_title, empty_detail) = snapshot
        .phase()
        .empty_result_copy(controller.validation().valid(), empty_workflow);

    rsx! {
        section { class: "workflow-results", "aria-label": "Workflow results",
            header { class: "workflow-results-meta",
                span { "{snapshot.phase().run_label()}" }
                if let Some(result) = &snapshot.result() {
                    span { class: "font-mono tabular-nums", "{result.rows().len()} rows · {result.elapsed_ms()} ms" }
                    if result.truncated() { span { class: "chip chip-caution", "Truncated" } }
                }
                if can_run_again && snapshot.result().is_some() {
                    Action {
                        purpose: ActionPurpose::Retry,
                        variant: ActionVariant::Link,
                        class: "workflow-results-retry",
                        onclick: move |_| controller.run(studio),
                        "Run again"
                    }
                }
            }
            if snapshot.result_stale() {
                div { class: "workflow-results-stale", role: "status",
                    strong { "Previous run" }
                    span { "{snapshot.phase().stale_result_copy()}" }
                }
            }
            if snapshot.result().is_some() && snapshot.error().is_some() {
                div { class: "workflow-results-error", role: "alert", "{snapshot.error().clone().unwrap_or_default()}" }
            }
            div { class: "workflow-result-body",
                if let Some(model) = result_model.or(transient_model) {
                    {model.render()}
                } else {
                    div { class: "workflow-panel-empty",
                        ResultNodeIcon {}
                        strong { "{empty_title}" }
                        p { "{empty_detail}" }
                        if can_run_again {
                            Action {
                                purpose: ActionPurpose::Retry,
                                variant: ActionVariant::Outline,
                                onclick: move |_| controller.run(studio),
                                "Run again"
                            }
                        }
                    }
                }
            }
        }
    }
}

trait WorkflowRunPresentation {
    fn badge_class(self, complete_with_warnings: bool) -> &'static str;
    fn empty_result_copy(
        self,
        configured: bool,
        empty_workflow: bool,
    ) -> (&'static str, &'static str);
    fn stale_result_copy(self) -> &'static str;
    fn run_label(self) -> &'static str;
}

impl WorkflowRunPresentation for WorkflowRunPhase {
    fn badge_class(self, complete_with_warnings: bool) -> &'static str {
        if complete_with_warnings {
            return "workflow-run-badge is-warning";
        }
        match self {
            WorkflowRunPhase::Validating
            | WorkflowRunPhase::Running
            | WorkflowRunPhase::Cancelling => "workflow-run-badge is-active",
            WorkflowRunPhase::Completed => "workflow-run-badge is-complete",
            WorkflowRunPhase::Failed => "workflow-run-badge is-failed",
            WorkflowRunPhase::Idle
            | WorkflowRunPhase::Cancelled
            | WorkflowRunPhase::Interrupted => "workflow-run-badge",
        }
    }

    fn empty_result_copy(
        self,
        configured: bool,
        empty_workflow: bool,
    ) -> (&'static str, &'static str) {
        if empty_workflow {
            return (
                "No workflow yet",
                "Ask the Assistant to build a workflow. Results will appear here after it runs.",
            );
        }
        match self {
            WorkflowRunPhase::Idle if configured => (
                "Ready for a result",
                "Run the saved graph from the canvas to produce authoritative rows.",
            ),
            WorkflowRunPhase::Idle => (
                "Configure the workflow",
                "Choose qualified Spiral relations and fix graph validation before running.",
            ),
            WorkflowRunPhase::Cancelled => (
                "Run cancelled",
                "No terminal result was published. Uncommitted feature work was discarded; any cache publication that completed before cancellation remains authoritative.",
            ),
            WorkflowRunPhase::Interrupted => (
                "Run interrupted",
                "Studio recovered an unfinished run without pretending it completed. The saved graph is unchanged.",
            ),
            WorkflowRunPhase::Failed => (
                "Run failed",
                "Review the affected node, then run the unchanged saved graph again.",
            ),
            WorkflowRunPhase::Completed => (
                "No terminal payload",
                "The run completed without publishing a Result payload.",
            ),
            WorkflowRunPhase::Validating
            | WorkflowRunPhase::Running
            | WorkflowRunPhase::Cancelling => (
                "Workflow in progress",
                "Results will appear when the backend publishes the terminal payload.",
            ),
        }
    }

    fn stale_result_copy(self) -> &'static str {
        match self {
            WorkflowRunPhase::Cancelled => {
                "The latest run was cancelled. These rows remain evidence from the previous completed run."
            }
            WorkflowRunPhase::Interrupted => {
                "The latest run was interrupted. These rows remain evidence from the previous completed run."
            }
            WorkflowRunPhase::Failed => {
                "The latest run failed. These rows remain evidence from the previous completed run."
            }
            _ => "These rows remain stale until the current saved graph completes.",
        }
    }

    fn run_label(self) -> &'static str {
        match self {
            WorkflowRunPhase::Idle => "Saved",
            WorkflowRunPhase::Validating => "Validating",
            WorkflowRunPhase::Running => "Running",
            WorkflowRunPhase::Cancelling => "Cancelling",
            WorkflowRunPhase::Completed => "Complete",
            WorkflowRunPhase::Failed => "Failed",
            WorkflowRunPhase::Cancelled => "Cancelled",
            WorkflowRunPhase::Interrupted => "Interrupted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_tiles<'a>(node: &'a LayoutNode, out: &mut Vec<(&'a str, Vec<&'a str>)>) {
        match node {
            LayoutNode::Tile(tile) => out.push((
                tile.id.as_str(),
                tile.panels.iter().map(PanelId::as_str).collect(),
            )),
            LayoutNode::Split { first, second, .. } => {
                collect_tiles(first, out);
                collect_tiles(second, out);
            }
        }
    }

    #[test]
    fn workflow_registers_the_approved_four_panel_composition() {
        let layout = WorkflowWorkspaceLayout::initial();
        let mut tiles = Vec::new();
        collect_tiles(&layout.root, &mut tiles);
        assert_eq!(
            tiles,
            vec![
                ("workflow-left", vec![ASSISTANT_PANEL_ID, NODE_PANEL_ID]),
                ("workflow-canvas", vec![CANVAS_PANEL_ID]),
                ("workflow-results", vec![RESULTS_PANEL_ID]),
            ]
        );
        match &layout.root {
            LayoutNode::Split {
                axis,
                ratio,
                second,
                ..
            } => {
                assert_eq!(*axis, SplitAxis::Horizontal);
                assert!((*ratio - 0.30).abs() < f64::EPSILON);
                let LayoutNode::Split { axis, ratio, .. } = second.as_ref() else {
                    panic!("workflow right side must split canvas and Results");
                };
                assert_eq!(*axis, SplitAxis::Vertical);
                assert!((*ratio - 0.66).abs() < f64::EPSILON);
            }
            LayoutNode::Tile(_) => panic!("workflow root must be a split"),
        }
    }

    #[test]
    fn interaction_language_keeps_run_authority_out_of_assistant() {
        let source = include_str!("workspace.rs");
        let source = source.split("#[cfg(test)]").next().unwrap();
        assert!(source.contains("Workflow Assistant"));
        assert!(source.contains("Enter to send · Shift+Enter for a new line."));
        assert!(source.contains("Jump to latest"));
        assert!(source.contains("ActionPurpose::Stop"));
        assert!(source.contains("Retry last turn"));
        assert_eq!(source.matches("Run workflow").count(), 1);
        assert_eq!(source.matches("Remove model").count(), 1);
        assert!(source.contains("ActionPurpose::Remove"));
        assert!(source.contains("LayoutDirection::LeftToRight"));
        assert!(source.contains("min_zoom: WORKFLOW_MIN_ZOOM"));
        assert!(source.contains("viewport.zoom >= WORKFLOW_MIN_ZOOM"));
        assert!(source.contains(".gaps(32.0, 56.0)"));
        assert!(source.contains("let mut canvas_topology = use_signal(String::new)"));
        assert!(source.contains("workflow-edit-badge"));
        assert!(source.contains("workflow-run-badge"));
        assert!(source.contains("Run again"));
        assert!(source.contains("Choose a qualified relation"));
        assert!(!source.contains("Hugging Face"));
        assert!(!source.contains("MCAP"));
    }

    #[test]
    fn assistant_parser_handles_safe_structure_and_incomplete_streams() {
        assert_eq!(
            AssistantDocument::parse(
                "Saved **Holding evidence** with `score_cube`.\n\n- Camera frames\n* Joint state\n\n```sql\nSELECT * FROM scores\n```"
            )
            .into_blocks(),
            vec![
                AssistantBlock::Paragraph(vec![
                    AssistantInline::Text("Saved ".to_owned()),
                    AssistantInline::Strong("Holding evidence".to_owned()),
                    AssistantInline::Text(" with ".to_owned()),
                    AssistantInline::Code("score_cube".to_owned()),
                    AssistantInline::Text(".".to_owned()),
                ]),
                AssistantBlock::List(vec![
                    vec![AssistantInline::Text("Camera frames".to_owned())],
                    vec![AssistantInline::Text("Joint state".to_owned())],
                ]),
                AssistantBlock::Code {
                    language: Some("sql".to_owned()),
                    text: "SELECT * FROM scores".to_owned(),
                },
            ]
        );
        assert_eq!(
            AssistantDocument::parse("Streaming **unfinished").into_blocks(),
            vec![AssistantBlock::Paragraph(vec![AssistantInline::Text(
                "Streaming **unfinished".to_owned()
            )])]
        );
        assert_eq!(
            AssistantDocument::parse("```sql\nSELECT 1").into_blocks(),
            vec![AssistantBlock::Code {
                language: Some("sql".to_owned()),
                text: "SELECT 1".to_owned(),
            }]
        );
    }

    #[test]
    fn conversation_and_tool_summaries_stay_specific_without_exposing_provider_plumbing() {
        let conversation =
            WorkflowAgentConversationSummary::new("", "Gripper evidence", 0).with_activity(0, 4);
        assert_eq!(conversation.option_label(), "Gripper evidence · 4 messages");
        let tools = [WorkflowAgentToolSummary::new(
            "call-1",
            "get_relations",
            "Inspecting camera relations",
        )];
        assert_eq!(
            tools.trace_summary(WorkflowAgentMessagePhase::Streaming),
            "Working · Inspecting camera relations"
        );
        let mut recovered_tool =
            WorkflowAgentToolSummary::new("call-2", "run_query", "Couldn’t inspect query results");
        recovered_tool.set_phase(WorkflowAgentToolPhase::Failed);
        recovered_tool.set_error(Some("Unsupported extension".to_owned()));
        let recovered = [recovered_tool];
        assert_eq!(
            recovered.trace_summary(WorkflowAgentMessagePhase::Complete),
            "1 step · 1 recovered"
        );
        let production = include_str!("workspace.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production.contains("LLM_API_KEY"));
        assert!(!production.contains("Configure the backend provider"));
        assert!(!production.contains("readiness.provider"));
        assert!(!production.contains("readiness.model"));
    }

    #[test]
    fn composer_draft_clears_only_after_the_matching_scoped_turn_is_accepted() {
        let mut drafts = BTreeMap::from([
            (
                AgentDraftKey::new("project-a", "conversation-a"),
                "first request".to_owned(),
            ),
            (
                AgentDraftKey::new("project-b", "conversation-a"),
                "other project draft".to_owned(),
            ),
            (
                AgentDraftKey::new("project-a", "conversation-b"),
                "other draft".to_owned(),
            ),
        ]);
        // A project cutover can legitimately load the same durable
        // conversation identity. The active scope selects only its own draft,
        // while returning to the original scope restores the untouched text.
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-a", "conversation-a"))
                .unwrap(),
            "first request"
        );
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-b", "conversation-a"))
                .unwrap(),
            "other project draft"
        );
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-a", "conversation-a"))
                .unwrap(),
            "first request"
        );
        assert!(
            !AcceptedAgentDraft::new("project-a", "conversation-a", "stale request")
                .clear_from(&mut drafts)
        );
        assert_eq!(drafts.len(), 3);
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-b", "conversation-a"))
                .unwrap(),
            "other project draft",
            "a rejected/stale project-a acceptance cannot clear project-b text"
        );
        assert!(
            AcceptedAgentDraft::new("project-a", "conversation-a", "first request")
                .clear_from(&mut drafts)
        );
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-b", "conversation-a"))
                .unwrap(),
            "other project draft"
        );
        assert_eq!(
            drafts
                .get(&AgentDraftKey::new("project-a", "conversation-b"))
                .unwrap(),
            "other draft"
        );
    }

    #[test]
    fn conversation_drafts_are_hoisted_above_route_and_settings_unmounts() {
        let state = include_str!("state.rs");
        let workspace = include_str!("workspace.rs");
        let production_workspace = workspace.split_once("#[cfg(test)]").unwrap().0;
        let routes = include_str!("../../shell/routes.rs");
        assert!(state.contains("agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>"));
        assert!(state.contains("agent_drafts: use_signal(BTreeMap::new)"));
        assert!(workspace.contains("let mut drafts = controller.agent_drafts()"));
        assert!(!production_workspace.contains("use_signal(BTreeMap::<String, String>::new)"));
        assert!(
            routes.find("use_workflow_controller();").unwrap()
                < routes.find("Outlet::<StudioRoute>").unwrap(),
            "the shell-owned controller must outlive the routed Workflow view"
        );
    }

    #[test]
    fn run_badges_never_color_cancelled_or_interrupted_as_success() {
        assert_eq!(
            WorkflowRunPhase::Cancelled.badge_class(false),
            "workflow-run-badge"
        );
        assert_eq!(
            WorkflowRunPhase::Interrupted.badge_class(false),
            "workflow-run-badge"
        );
        assert_eq!(
            WorkflowRunPhase::Completed.badge_class(false),
            "workflow-run-badge is-complete"
        );
        assert_eq!(
            WorkflowRunPhase::Completed.badge_class(true),
            "workflow-run-badge is-warning"
        );
    }
}
