//! Workflow conversation, execution canvas, and dedicated node inspector.

use std::rc::Rc;

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::workflow::{
    WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentRole, WorkflowAgentToolPhase,
    WorkflowAgentToolSummary, WorkflowRunPhase,
};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID, NODE_PANEL_ID,
    RESULTS_PANEL_ID, WorkflowPendingCommand,
};
use crate::components::icons::{ResultNodeIcon, WorkflowIcon};
use crate::shell::state::StudioProjectState;

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-sql-plan-v6";

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
    let panels = vec![
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-left",
            rsx! { WorkflowAssistant { controller } },
        )
        .with_class("workflow-assistant-editor"),
        Panel::new(
            CANVAS_PANEL_ID,
            "Query plan",
            "workflow-canvas",
            rsx! { WorkflowCanvas { controller } },
        )
        .with_class("workflow-canvas-editor"),
        Panel::new(
            NODE_PANEL_ID,
            "SQL",
            "workflow-inspector",
            rsx! { WorkflowSqlEditor { controller } },
        )
        .with_class("workflow-node-editor"),
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
            super::document_controls::WorkflowDocumentControls { controller }
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

struct WorkflowWorkspaceLayout;

impl WorkflowWorkspaceLayout {
    fn initial() -> PanelLayout {
        let right = LayoutNode::split(
            "workflow-right-rows",
            SplitAxis::Vertical,
            0.60,
            LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
            LayoutNode::split(
                "workflow-details-columns",
                SplitAxis::Horizontal,
                0.50,
                LayoutNode::tile("workflow-inspector", [NODE_PANEL_ID]),
                LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
            ),
        );
        PanelLayout::new(LayoutNode::split(
            "workflow-columns",
            SplitAxis::Horizontal,
            0.30,
            LayoutNode::tile("workflow-left", [ASSISTANT_PANEL_ID]),
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
    let can_send = configured
        && controller.loaded()
        && state.history_available()
        && controller.agent_loaded()
        && !active
        && command.is_none()
        && catalog_available
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
                    h2 { class: "sr-only", "Assistant" }
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
                        p { "Describe what to find. I’ll inspect the data and build a workflow for you to run. Click Save to keep your work." }
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
                                        "data-failed": message.phase() != WorkflowAgentMessagePhase::Complete && message.tools().iter().any(|tool| tool.phase() == WorkflowAgentToolPhase::Failed),
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
                            title: if !catalog_available { "Reconnect Spiral before sending a message" } else if !configured { "Assistant is temporarily unavailable" } else { "Send this message to the Workflow Assistant" },
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
fn WorkflowSqlEditor(controller: super::state::WorkflowController) -> Element {
    let spec = controller.spec();
    let validation = controller.validation();
    rsx! {
        section { class: "workflow-sql-editor",
            label { r#for: "workflow-sql", class: "label", "Query" }
            textarea {
                id: "workflow-sql", class: "field workflow-sql-input", spellcheck: "false",
                value: spec.sql(), placeholder: "SELECT …",
                oninput: move |event| controller.set_sql(event.value()),
            }
            if !spec.sql().trim().is_empty() {
                for issue in validation.errors() {
                    p { class: "workflow-results-error", "{issue.message()}" }
                }
            }
        }
    }
}

#[component]
fn WorkflowCanvas(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let spec = controller.spec();
    let snapshot = controller.snapshot();
    let dirty = controller.unsaved_changes();
    let command = controller.command();
    let run_reason = controller.run_disabled_reason();
    let save_reason = controller.save_disabled_reason();
    let plan = use_resource(move || {
        let expected = controller.spec();
        async move {
            if expected.sql().trim().is_empty() {
                return (expected, Ok(None));
            }
            gloo_timers::future::TimeoutFuture::new(350).await;
            let result = super::WorkflowClient::current()
                .plan(&expected)
                .await
                .map(Some)
                .map_err(|error| error.to_string());
            (expected, result)
        }
    });
    let current = plan
        .read()
        .as_ref()
        .filter(|(expected, _)| expected == &spec)
        .map(|(_, result)| result.clone());
    rsx! {
        section { class: "workflow-canvas", "aria-label": "Native query plan",
            header { class: "workflow-canvas-toolbar",
                div {
                    strong { class: if dirty { "workflow-edit-badge is-modified" } else { "workflow-edit-badge" },
                        if dirty { "Unsaved changes" } else { "Saved" }
                    }
                    if snapshot.phase() != WorkflowRunPhase::Idle {
                        strong { class: snapshot.phase().badge_class(false), "{snapshot.phase().run_label()}" }
                    }
                }
                if controller.revision_conflict() {
                    Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                        onclick: move |_| controller.reload_current(), "Reload current" }
                } else if dirty {
                    Action { purpose: ActionPurpose::Sync, variant: ActionVariant::Outline,
                        disabled: command.is_some() || controller.agent_state().session().phase().active(),
                        onclick: move |_| controller.discard(studio), "Discard changes" }
                }
                Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline,
                    disabled: save_reason.is_some(), title: save_reason.unwrap_or_else(|| "Save SQL and conversation".into()),
                    onclick: move |_| controller.save(studio),
                    if command == Some(WorkflowPendingCommand::Saving) { "Saving…" } else { "Save" }
                }
                Action { purpose: ActionPurpose::Start, variant: ActionVariant::Solid,
                    disabled: run_reason.is_some(), title: run_reason.unwrap_or_else(|| "Run this SQL in Spiral".into()),
                    onclick: move |_| controller.run(studio),
                    if snapshot.phase().active() { "Running…" } else { "Run" }
                }
            }
            if let Some(error) = controller.load_error() {
                div { class: "workflow-results-error", role: "alert", "{error}" }
            }
            if spec.sql().trim().is_empty() {
                div { class: "workflow-panel-empty", WorkflowIcon {}, strong { "Describe a query" }
                    p { "Ask the Assistant or write SQL. Spiral’s query plan will appear here." }
                }
            } else {
                match current {
                    None => rsx! { div { class: "workflow-panel-empty", role: "status", "Planning SQL…" } },
                    Some(Err(error)) => rsx! { div { class: "workflow-results-error", role: "alert", "{error}" } },
                    Some(Ok(Some(plan))) => rsx! {
                        if let Some(error) = plan.error() {
                            div { class: "workflow-results-error", role: "alert", "{error}" }
                        }
                        if let Some(report) = plan.report() {
                            super::plan_display::NativePlanDisplay { key: "{spec.id()}:{spec.sql()}", report: report.clone() }
                        }
                    },
                    Some(Ok(None)) => rsx! {},
                }
            }
        }
    }
}

#[component]
fn WorkflowResults(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let snapshot = controller.snapshot();
    let run_reason = controller.run_disabled_reason();
    let can_run_again = matches!(
        snapshot.phase(),
        WorkflowRunPhase::Interrupted | WorkflowRunPhase::Failed
    ) && run_reason.is_none();
    let spec = controller.spec();

    let empty_workflow = spec.sql().trim().is_empty();
    let executed = controller.executed_query().unwrap_or_else(|| spec.clone());
    let sql = executed.sql().to_owned();
    let max_rows = 100;
    let result_model = snapshot.result().cloned().map(|result| {
        QueryExecutionPresentation::new(sql.clone(), max_rows, QueryState::Ready(result))
    });
    let transient_model = if result_model.is_none() {
        match snapshot.phase() {
            WorkflowRunPhase::Validating | WorkflowRunPhase::Running => Some(
                QueryExecutionPresentation::new(sql.clone(), max_rows, QueryState::Running),
            ),
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
            if let Some(result) = snapshot.result() {
                header { class: "workflow-results-meta",
                    span { "{snapshot.phase().run_label()}" }
                    span { class: "font-mono tabular-nums", "{result.rows().len()} rows · {result.elapsed_ms()} ms" }
                    if result.truncated() { span { class: "chip chip-caution", "Truncated" } }
                    if can_run_again {
                        Action {
                            purpose: ActionPurpose::Retry,
                            variant: ActionVariant::Link,
                            class: "workflow-results-retry",
                            onclick: move |_| controller.run(studio),
                            "Run again"
                        }
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
            WorkflowRunPhase::Validating | WorkflowRunPhase::Running => {
                "workflow-run-badge is-active"
            }
            WorkflowRunPhase::Completed => "workflow-run-badge is-complete",
            WorkflowRunPhase::Failed => "workflow-run-badge is-failed",
            WorkflowRunPhase::Idle | WorkflowRunPhase::Interrupted => "workflow-run-badge",
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
            WorkflowRunPhase::Idle if configured => {
                ("No results yet", "Run workflow to see results here.")
            }
            WorkflowRunPhase::Idle => (
                "Configure the workflow",
                "Choose qualified Spiral relations and fix SQL validation before running.",
            ),
            WorkflowRunPhase::Interrupted => (
                "Run interrupted",
                "Studio recovered an unfinished run without pretending it completed. The submitted SQL is unchanged.",
            ),
            WorkflowRunPhase::Failed => (
                "Run failed",
                "Review the SQL error, then run the query again.",
            ),
            WorkflowRunPhase::Completed => (
                "No terminal payload",
                "The run completed without publishing a Result payload.",
            ),
            WorkflowRunPhase::Validating | WorkflowRunPhase::Running => (
                "Workflow in progress",
                "Results will appear when the backend publishes the terminal payload.",
            ),
        }
    }

    fn stale_result_copy(self) -> &'static str {
        match self {
            WorkflowRunPhase::Interrupted => {
                "The latest run was interrupted. These rows remain evidence from the previous completed run."
            }
            WorkflowRunPhase::Failed => {
                "The latest run failed. These rows remain evidence from the previous completed run."
            }
            _ => "These rows remain stale until the current query completes.",
        }
    }

    fn run_label(self) -> &'static str {
        match self {
            WorkflowRunPhase::Idle => "Ready",
            WorkflowRunPhase::Validating => "Validating",
            WorkflowRunPhase::Running => "Running",
            WorkflowRunPhase::Completed => "Complete",
            WorkflowRunPhase::Failed => "Failed",
            WorkflowRunPhase::Interrupted => "Interrupted",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

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
    fn run_badges_never_color_interrupted_as_success() {
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
    let controller = super::state::WorkflowController::use_controller();
    controller.use_polling(is_active);
    let status = use_memo(move || controller.editor_status());
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Workflow(status), WorkflowPanels { controller } } }
}
