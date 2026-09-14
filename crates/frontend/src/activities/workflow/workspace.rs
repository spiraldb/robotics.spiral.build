//! Workflow conversation, execution canvas, and dedicated node inspector.

use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::workflow::{WorkflowAgentPhase, WorkflowRunPhase};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID, NODE_PANEL_ID,
    RESULTS_PANEL_ID,
};
use crate::components::icons::{ResultNodeIcon, WorkflowIcon};
use crate::shell::state::StudioProjectState;

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-investigation-v8";

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
            "Graph",
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
            0.45,
            LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
            LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
        );
        PanelLayout::new(LayoutNode::split(
            "workflow-columns",
            SplitAxis::Horizontal,
            0.28,
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
    let mut thread_scroll = use_signal(|| None::<web_sys::Element>);
    let mut drafts = controller.agent_drafts();
    let command = controller.agent_command();
    let active = state.session().phase().active();
    let configured = state.readiness().configured();
    let draft_key = AgentDraftKey::new(state.session().workflow_id());
    let draft = drafts.read().get(&draft_key).cloned().unwrap_or_default();
    let catalog_available =
        crate::shell::project::ProjectSessionSignals::current().catalog_available();
    let loading = !controller.loaded() && controller.load_error().is_none();
    let can_send = configured
        && controller.loaded()
        && state.history_available()
        && controller.agent_loaded()
        && !active
        && command.is_none()
        && catalog_available
        && !draft.trim().is_empty();
    let status = if loading {
        "Loading workflow…"
    } else if !state.history_available() {
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
        let scroll = thread_scroll.peek().to_owned();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(24).await;
            if *follow_latest.peek()
                && let Some(scroll) = scroll
            {
                scroll.set_scroll_top(scroll.scroll_height());
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
            if active && controller.workflow_runs().iter().any(|run| run.snapshot().phase().active()) {
                p { class: "workflow-plan-node-id", role: "status", "Spiral is running a query. Stopping the Assistant leaves that query running." }
            }
            div {
                id: "workflow-assistant-scroll",
                class: "workflow-assistant-scroll",
                onmounted: move |event| {
                    if let Some(element) = event.data().downcast::<web_sys::Element>() {
                        thread_scroll.set(Some(element.clone()));
                    }
                },
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
                        p { "Describe the result you need. I’ll inspect the data, run queries, and refine the results. Click Save to keep the SQL and conversation." }
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
                    super::assistant_content::AssistantConversation { messages, controller }
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
                            let scroll = thread_scroll.peek().to_owned();
                            if let Some(scroll) = scroll {
                                scroll.set_scroll_top(scroll.scroll_height());
                            }
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
                    span { role: "status",
                        if loading { "Loading workflow…" } else { "{composer_hint}" }
                    }
                    if active {
                        Action {
                            purpose: ActionPurpose::Stop,
                            variant: ActionVariant::Outline,
                            disabled: command == Some(AgentPendingCommand::Cancelling),
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
    let spec = controller.spec();
    // A rename or save revision cannot change a SQL plan. Subscribe to the
    // actual query so those actions neither clear the graph nor replan it.
    let query = use_memo(move || {
        let spec = controller.spec();
        WorkflowPlanInput(spec)
    });
    let mut previous_query: Option<WorkflowPlanInput> = None;
    let plan = use_resource(move || {
        let expected = query();
        let editing = previous_query
            .as_ref()
            .is_some_and(|previous| previous.0.id() == expected.0.id());
        previous_query = Some(expected.clone());
        async move {
            if expected.0.sql().trim().is_empty() {
                return (expected, Ok(None));
            }
            // Debounce typing, but request an opened document immediately.
            if editing {
                gloo_timers::future::TimeoutFuture::new(350).await;
            }
            let result = super::WorkflowClient::current()
                .plan(&expected.0)
                .await
                .map(Some)
                .map_err(|error| error.to_string());
            (expected, result)
        }
    });
    let current = plan
        .read()
        .as_ref()
        .filter(|(expected, _)| expected == &*query.read())
        .map(|(_, result)| result.clone());
    rsx! {
        section { class: "workflow-canvas", "aria-label": "Native query plan",

            if let Some(error) = controller.load_error() {
                div { class: "workflow-results-error", role: "alert", "{error}" }
            } else if !controller.loaded() {
                div { class: "workflow-panel-empty", role: "status", "Loading workflow…" }
            } else if spec.sql().trim().is_empty() {
                div { class: "workflow-panel-empty", WorkflowIcon {}, strong { "Build a workflow" }
                    p { "Ask the Assistant or write SQL. Explore the resulting graph here, then inspect the rows below." }
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

/// Document metadata travels with the request, but only identity and SQL
/// invalidate its plan. A save or rename leaves an in-flight request valid.
#[derive(Clone)]
struct WorkflowPlanInput(spiral_robotics_common::workflow::WorkflowSpec);

impl PartialEq for WorkflowPlanInput {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id() && self.0.sql() == other.0.sql()
    }
}

struct RunDuration(u64);

impl RunDuration {
    fn display(&self) -> String {
        if self.0 < 1000 {
            format!("{} ms", self.0)
        } else {
            format!("{:.1} s", self.0 as f64 / 1000.0)
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
    let runs = controller.workflow_runs();

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
            if !runs.is_empty() {
                div { class: "workflow-results-toolbar",
                    label { r#for: "workflow-result-run", "Run" }
                    select { id: "workflow-result-run", value: snapshot.run_id().unwrap_or_default(),
                        onchange: move |event| controller.select_run(event.value()),
                        for (index, run) in runs.iter().enumerate() {
                            option { key: "{run.snapshot().run_id().unwrap_or_default()}",
                                value: run.snapshot().run_id().unwrap_or_default(),
                                selected: run.snapshot().run_id() == snapshot.run_id(),
                                "{index + 1} · {run.snapshot().phase().run_label()}"
                            }
                        }
                    }
                    if let Some(result) = snapshot.result() {
                        strong { class: "workflow-result-count", "{result.rows().len()} rows" }
                        span { class: "workflow-result-duration", "{RunDuration(result.elapsed_ms()).display()}" }
                        if result.truncated() { span { class: "chip chip-caution", "Truncated" } }
                    }
                    details { class: "workflow-executed-sql", summary { "Executed SQL" } pre { "{sql}" } }
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
            div { key: "{snapshot.run_id().unwrap_or_default()}", class: "workflow-result-body",
                if !controller.loaded() && controller.load_error().is_none() {
                    div { class: "workflow-panel-empty", role: "status", "Loading workflow…" }
                } else if let Some(model) = result_model.or(transient_model) {
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
    fn empty_result_copy(
        self,
        configured: bool,
        empty_workflow: bool,
    ) -> (&'static str, &'static str);
    fn stale_result_copy(self) -> &'static str;
    fn run_label(self) -> &'static str;
}

impl WorkflowRunPresentation for WorkflowRunPhase {
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
