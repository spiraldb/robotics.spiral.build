//! Four-panel relation Workflow editor, execution canvas, and real results.

use std::rc::Rc;

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_flow::{
    Background, BackgroundVariant, Flow, FlowHandle, Handle, HandleKind, LayoutDirection,
    LayoutOptions, NodeViewCtx, Point, use_flow_handle,
};
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::workflow::{
    WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentRole, WorkflowAgentToolPhase,
    WorkflowAgentToolSummary, WorkflowNodeConfig, WorkflowNodePhase, WorkflowRunPhase,
};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID, ModelFeatureTarget,
    NODE_PANEL_ID, RESULTS_PANEL_ID, WorkflowNodeData, WorkflowNodeKind, WorkflowPendingCommand,
};
use crate::components::icons::{
    FitViewIcon, ModelNodeIcon, QueryNodeIcon, ResultNodeIcon, TableNodeIcon, WorkflowIcon,
};
use crate::shell::state::StudioProjectState;

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
fn WorkflowPanels(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let workflows = controller.workflows();
    let selected = controller.spec().id().to_owned();
    let reason = controller.workflow_selection_reason();
    let panels = vec![
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-left",
            rsx! { WorkflowAssistant { controller } },
        )
        .with_class("workflow-assistant-editor"),
        Panel::new(
            NODE_PANEL_ID,
            "Node",
            "workflow-left",
            rsx! { WorkflowNodeInspector { controller } },
        )
        .with_class("workflow-node-editor"),
        Panel::new(
            CANVAS_PANEL_ID,
            "Workflow",
            "workflow-canvas",
            rsx! { WorkflowCanvas { controller } },
        )
        .with_class("workflow-canvas-editor"),
        Panel::new(
            RESULTS_PANEL_ID,
            "Results",
            "workflow-results",
            rsx! { WorkflowResults { controller } },
        )
        .with_class("workflow-results-editor"),
    ];

    rsx! {
        div { class: "workflow-workspace",
            header { class: "workflow-picker",
                label { r#for: "workflow-picker", "Workflow" }
                select {
                    id: "workflow-picker",
                    value: "{selected}",
                    disabled: reason.is_some(),
                    onchange: move |event| controller.select_workflow(Some(event.value()), studio),
                    for workflow in workflows {
                        option { value: "{workflow.id()}", selected: workflow.id() == selected, "{workflow.name()}" }
                    }
                }
                Action {
                    purpose: ActionPurpose::Create,
                    variant: ActionVariant::Outline,
                    disabled: reason.is_some(),
                    onclick: move |_| controller.select_workflow(None, studio),
                    "New workflow"
                }
                WorkflowDatasetPicker { controller }
                if let Some(reason) = reason.as_ref() { span { role: "status", "{reason}" } }
            }
            div { class: "workflow-panels",
        PanelWorkspace {
            panels,
            initial_layout: WorkflowWorkspaceLayout::initial(),
            layout_scope: Some(WORKFLOW_LAYOUT_SCOPE.to_owned()),
            active_panel: controller.active_panel(),
            on_panel_activate: move |id: PanelId| controller.activate_panel(id),
        }
            }
        }
    }
}

#[component]
fn WorkflowDatasetPicker(controller: super::state::WorkflowController) -> Element {
    use crate::activities::data::DatasetClient;
    use crate::client::queries::{QueryDomain, QueryPolicy};
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let datasets = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
        move || sessions.catalog_available(),
        || DatasetClient::current().list_datasets(),
    );
    let studio = StudioProjectState::current();
    let selected = String::new();
    let response = datasets.read();
    match response.as_ref() {
        Some(Ok(items)) => {
            let items = items.clone();
            let choices = items.clone();
            let empty = items.is_empty();
            rsx! {
                label { r#for: "workflow-dataset", "Start from dataset" }
                select {
                    id: "workflow-dataset", value: "{selected}",
                    disabled: controller.workflow_selection_reason().is_some() || empty,
                    title: "Start a workflow from this dataset. Replaces the current graph.",
                    onchange: move |event| {
                        if let Some(source) = choices.iter().find(|source| source.resource_id() == event.value()) {
                            controller.select_dataset(source.clone(), studio);
                        }
                    },
                    option { value: "", disabled: true, if empty { "No datasets available" } else { "Choose a dataset" } }
                    for source in items {
                        option { value: "{source.resource_id()}", selected: source.resource_id() == selected, "{source.selection().label()}" }
                    }
                }
            }
        }
        Some(Err(error)) => rsx! { span { role: "alert", "Could not load datasets: {error}" } },
        None => rsx! { span { role: "status", "Loading datasets…" } },
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
fn WorkflowAssistant(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let state = controller.agent_state();
    let mut follow_latest = use_signal(|| true);
    let mut show_latest_action = use_signal(|| false);
    let mut thread_tail = use_signal(|| None::<Rc<MountedData>>);
    let mut drafts = controller.agent_drafts();
    let command = controller.agent_command();
    let active = state.session().phase().active();
    let configured = state.readiness().configured();
    let draft_key = AgentDraftKey::new(state.session().workflow_id());
    let draft = drafts.read().get(&draft_key).cloned().unwrap_or_default();
    let catalog_available =
        crate::shell::project::ProjectSessionSignals::current().catalog_available();
    let workflow_active = controller.snapshot().phase().active();
    let can_send = configured
        && controller.loaded()
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
                        p { "Describe what to find. I’ll inspect the data and save a workflow for you to run." }
                        if configured {
                            div { class: "workflow-assistant-examples",
                                Action {
                                    purpose: ActionPurpose::Select,
                                    variant: ActionVariant::Outline,
                                    onclick: {
                                        let draft_key = starter_relations_key.clone();
                                        move |_| {
                                            drafts.write().insert(draft_key.clone(), ("Select all episodes but remove the ones whose top camera is too dim.").to_owned());
                                        }
                                    },
                                    "Remove episodes with dim top-camera footage"
                                }
                                Action {
                                    purpose: ActionPurpose::Select,
                                    variant: ActionVariant::Outline,
                                    onclick: {
                                        let draft_key = starter_siglip_key.clone();
                                        move |_| {
                                            drafts.write().insert(draft_key.clone(), ("Find moments when the gripper holds an object.").to_owned());
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
                                    {AssistantText::parse(message.text()).render()}
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
struct AssistantText(Vec<AssistantBlock>);

impl AssistantText {
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
fn WorkflowNodeInspector(controller: super::state::WorkflowController) -> Element {
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
                            "Reference each connected input with an exact placeholder such as "
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
                            "Choose a decoded image column from the parent node. SigLIP appends one score per concept and preserves the input rows. Every Run computes the scores again."
                        }
                        div { class: "workflow-feature-definitions",
                            label {
                                span { class: "label", "Input image column" }
                                input {
                                    class: "field font-mono", disabled: active, value: data.image_column(),
                                    oninput: { let node_id = node_id.clone(); move |event| controller.set_model_image(&node_id, event.value()) },
                                }
                            }
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

                                }
                            }
                        }
                        div { class: "workflow-inspector-actions",
                            Action {
                                purpose: ActionPurpose::Create,
                                variant: ActionVariant::Outline,
                                disabled: active,
                                title: "Add another output score",
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
                                title: if snapshot_active { "Cancel the active run before removing this Model node" } else if command_pending { "Wait for the current Workflow command before removing this Model node" } else { "Remove this Model and reconnect its input to the downstream nodes" },
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
                {node_validation_issues(controller, &node_id)}
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

fn node_validation_issues(controller: super::state::WorkflowController, node_id: &str) -> Element {
    let validation = controller.validation();
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
fn WorkflowCanvas(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
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
    let state_load_failed = controller.load_error().is_some();
    let dirty = controller.dirty();
    let empty = spec.nodes().is_empty() && spec.edges().is_empty();
    let edit_label = if dirty {
        "Modified".to_owned()
    } else {
        "Saved".to_owned()
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
        section { class: "workflow-canvas", "aria-label": "Workflow graph",
            header { class: "workflow-canvas-toolbar",
                input {
                    class: "workflow-name-input",
                    "aria-label": "Workflow name",
                    value: "{spec.name()}",
                    disabled: topology_blocked || controller.agent_state().session().phase().active(),
                    oninput: move |event| controller.set_name(event.value()),
                }
                div {
                    strong { class: if dirty { "workflow-edit-badge is-modified" } else { "workflow-edit-badge" }, "{edit_label}" }
                    if snapshot.phase() != WorkflowRunPhase::Idle {
                        strong { class: snapshot.phase().badge_class(complete_with_warnings), "{run_label}" }
                    }
                    span { "{spec.nodes().len()} nodes · {spec.edges().len()} connections" }
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
            if !matches!(data.kind(), WorkflowNodeKind::Table) {
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
fn WorkflowResults(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
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
                "No new result was published. Run again to recompute the workflow.",
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
    use std::collections::BTreeMap;

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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn assistant_parser_handles_safe_structure_and_incomplete_streams() {
        assert_eq!(
            AssistantText::parse(
                "Saved **Holding evidence** with `score_cube`.\n\n- Camera frames\n* Joint state\n\n```sql\nSELECT * FROM scores\n```"
            )
            .0,
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
            AssistantText::parse("Streaming **unfinished").0,
            vec![AssistantBlock::Paragraph(vec![AssistantInline::Text(
                "Streaming **unfinished".to_owned()
            )])]
        );
        assert_eq!(
            AssistantText::parse("```sql\nSELECT 1").0,
            vec![AssistantBlock::Code {
                language: Some("sql".to_owned()),
                text: "SELECT 1".to_owned(),
            }]
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn composer_draft_belongs_to_its_workflow_and_preserves_new_typing() {
        let first = AgentDraftKey::new("workflow-a");
        let second = AgentDraftKey::new("workflow-b");
        let mut drafts = BTreeMap::from([
            (first.clone(), "use top camera".to_owned()),
            (second.clone(), "use wrist camera".to_owned()),
        ]);
        assert!(!first.clear_if_unchanged(&mut drafts, "old request"));
        assert_eq!(
            drafts.get(&AgentDraftKey::new("workflow-a")).unwrap(),
            "use top camera"
        );
        assert!(first.clear_if_unchanged(&mut drafts, "use top camera"));
        assert_eq!(drafts.get(&second).unwrap(), "use wrist camera");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
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

#[component]
pub(crate) fn WorkflowWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Workflow,
        is_active,
    );
    let catalog = crate::activities::data::relations::RelationCatalogState::use_catalog();
    let controller = super::state::WorkflowController::use_controller(catalog.signal());
    controller.use_polling(is_active);
    let status = use_memo(move || controller.accepted_state());
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Workflow(status), WorkflowPanels { controller } } }
}
