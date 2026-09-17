//! Workflow investigation: the pipeline as the hero, its rows beneath it,
//! the conversation as a tall column at its side.
//!
//! Direction contract (user-pinned composition over seed b05dbd1e).
//! THESIS: the graph is how an engineer understands what a workflow does, so
//! it owns the center; results prove it beneath, and the Assistant authors it
//! from a narrow tall column. This refuses the debugger arrangement where a
//! plan is a strip of unreadable operators above a table.
//! OWN-WORLD: the one-light bench, unchanged: paper, graphite, hairlines, 4px
//! radii, 12/11/10px type, tabular mono numerals. Added: six stage-kind
//! identity tints as glyph tiles, categorical chips in soft washes, 2px value
//! bars under measurements, skeleton shimmer while Spiral runs, and one
//! staggered entrance that follows the data left to right.
//! STORY: ask, watch the query flow across the pipeline, see rows with images
//! arrive, read each stage's plain-language role, inspect exact values.
//! FIRST VIEWPORT: one document bar (title ▾ · state · Discard Save ▶Run);
//! left 24% Assistant | SQL; right: Graph (74%) beside Details (26%) over
//! Results (54% of the height).
//! FORM: graph hero with a side console and a results shelf.
//! FINISH: unreviewed and undocumented is unfinished; this build ends with the
//! finish review, the verdict, and DESIGN.md.

use crate::components::interaction::{Action, ActionPurpose, ActionVariant, ModeAction, ModeGroup};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::workflow::{WorkflowAgentPhase, WorkflowRunPhase};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID, NODE_PANEL_ID,
    RESULTS_PANEL_ID,
};
use crate::components::document_bar::ExecutedSql;
use crate::components::icons::{
    ConversationIcon, DetailsIcon, ResultNodeIcon, SendIcon, SpinnerIcon, SqlIcon, StopIcon,
    WorkflowIcon,
};
use crate::components::run_phase::RunPhasePresentation;
use crate::shell::state::StudioProjectState;

use super::highlight::SqlHighlight;

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-investigation-v12";
const STAGE_DETAILS_PANEL_ID: &str = "workflow-stage-details";

#[component]
fn WorkflowPanels(controller: super::state::WorkflowController) -> Element {
    let mut inspection = use_signal(|| None::<WorkflowInspection>);
    let mut comparing = use_signal(|| false);
    let mut displayed_workflow = use_signal(String::new);
    use_effect(move || {
        let spec = controller.spec();
        if *displayed_workflow.peek() != spec.id() && controller.loaded() {
            comparing.set(false);
            displayed_workflow.set(spec.id().to_owned());
        }
    });
    // A stage asked for from outside the canvas: the overview list or the
    // stage strip. The graph selects and centres it, then clears the request.
    let focus_stage = use_signal(|| None::<String>);
    let report = controller.planned_report();
    use_effect(move || {
        let query = WorkflowPlanInput(controller.spec());
        if inspection
            .peek()
            .as_ref()
            .is_some_and(|inspection| inspection.query != query)
        {
            inspection.set(None);
        }
    });
    let selection = inspection
        .read()
        .as_ref()
        .filter(|inspection| inspection.query == WorkflowPlanInput(controller.spec()))
        .map(|inspection| inspection.details.clone());
    let panels = vec![
        Panel::new(
            RESULTS_PANEL_ID,
            "Results",
            "workflow-results",
            rsx! { WorkflowResults { controller } },
        )
        .with_class("workflow-results-editor")
        .with_tab_icon(rsx! { ResultNodeIcon {} }),
        Panel::new(
            CANVAS_PANEL_ID,
            "Graph",
            "workflow-canvas",
            rsx! { WorkflowCanvas { controller, focus_stage,
                on_inspect: move |selected| {
                    inspection.set(Some(selected));
                    controller.activate_panel(STAGE_DETAILS_PANEL_ID.into());
                },
            } },
        )
        .with_class("workflow-canvas-editor")
        .with_tab_icon(rsx! { WorkflowIcon {} }),
        Panel::new(
            STAGE_DETAILS_PANEL_ID,
            "Details",
            "workflow-details",
            rsx! { super::plan_display::WorkflowGraphInspector { selection, report, focus_stage } },
        )
        .with_class("workflow-stage-details-editor")
        .with_tab_icon(rsx! { DetailsIcon {} }),
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-console",
            rsx! { WorkflowAssistant { controller } },
        )
        .with_class("workflow-assistant-editor")
        .with_tab_icon(rsx! { ConversationIcon {} }),
        Panel::new(
            NODE_PANEL_ID,
            "SQL",
            "workflow-console",
            rsx! { WorkflowSqlEditor { controller } },
        )
        .with_class("workflow-node-editor")
        .with_tab_icon(rsx! { SqlIcon {} }),
    ];

    rsx! {
        div { class: "workflow-workspace",
            if !comparing() { super::document_controls::WorkflowDocumentControls { controller, comparing } }
            div { class: if comparing() { "workflow-panels workflow-mode-hidden" } else { "workflow-panels" },
                PanelWorkspace {
                    panels,
                    initial_layout: WorkflowWorkspaceLayout::initial(),
                    layout_scope: Some(WORKFLOW_LAYOUT_SCOPE.to_owned()),
                    active_panel: controller.active_panel(),
                    on_panel_activate: move |id: PanelId| controller.activate_panel(id),
                }
            }
            div { class: if comparing() { "workflow-comparison-container" } else { "workflow-comparison-container workflow-mode-hidden" },
                super::comparison::WorkflowComparison { key: "{controller.spec().id()}", controller, comparing }
            }
        }
    }
}

/// Build authors one workflow; Compare measures two against each other. The
/// switch stands at the same place in both bars, so changing view never moves
/// the header.
#[component]
pub(super) fn WorkflowViewSwitch(comparing: Signal<bool>) -> Element {
    let mut comparing = comparing;
    rsx! {
        ModeGroup { label: "Workflow view".to_owned(), class: "workflow-view-switch".to_owned(),
            ModeAction { purpose: ActionPurpose::Select, checked: !comparing(),
                onclick: move |_| comparing.set(false), "Build" }
            ModeAction { purpose: ActionPurpose::Select, checked: comparing(),
                onclick: move |_| comparing.set(true), "Compare" }
        }
    }
}

struct WorkflowWorkspaceLayout;

impl WorkflowWorkspaceLayout {
    /// The graph owns the large central tile with its inspector beside it,
    /// results run along the bottom, and the Assistant and SQL share a tall
    /// column at the left. Every tile stays dockable and resizable.
    fn initial() -> PanelLayout {
        let explanation = LayoutNode::split(
            "workflow-explanation-columns",
            SplitAxis::Horizontal,
            0.74,
            LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
            LayoutNode::tile("workflow-details", [STAGE_DETAILS_PANEL_ID]),
        );
        // A pipeline is wider than it is tall: two stacked sources and a chain
        // need about 220px of canvas, so the graph row takes 46% of the height
        // and the results shelf keeps five rows of previews.
        let evidence = LayoutNode::split(
            "workflow-evidence-rows",
            SplitAxis::Vertical,
            0.46,
            explanation,
            LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
        );
        PanelLayout::new(LayoutNode::split(
            "workflow-columns",
            SplitAxis::Horizontal,
            0.24,
            LayoutNode::tile("workflow-console", [ASSISTANT_PANEL_ID, NODE_PANEL_ID]),
            evidence,
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
                    "Assistant ready"
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
    let query_running = controller
        .workflow_runs()
        .iter()
        .any(|run| run.snapshot().phase().active());
    let composer_disabled = !state.history_available()
        || !configured
        || active
        || command.is_some()
        || !catalog_available;
    let starter_relations_key = draft_key.clone();
    let starter_siglip_key = draft_key.clone();
    let submit_key = draft_key.clone();
    let input_key = draft_key.clone();
    let keydown_key = draft_key.clone();
    let send_key = draft_key;
    rsx! {
        section { class: "workflow-assistant", "aria-label": "Workflow assistant",
            div { class: "workflow-assistant-body",
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
                        p { "Describe the result you need. The Assistant inspects the data, writes and runs the SQL, and refines the rows until they answer the question. Save keeps the SQL and this conversation." }
                        if configured {
                            div { class: "workflow-assistant-examples",
                                Action {
                                    purpose: ActionPurpose::Select,
                                    variant: ActionVariant::Row,
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
                                    variant: ActionVariant::Row,
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
                div { class: "workflow-assistant-thread-tail", "aria-hidden": "true" }
            }
            if show_latest_action() {
                div { class: "workflow-assistant-jump",
                    Action {
                        purpose: ActionPurpose::Navigate,
                        variant: ActionVariant::Outline,
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
                if query_running {
                    p { class: "workflow-assistant-notice", role: "status",
                        if active { "A Spiral query is running. Stopping the Assistant does not stop the query." }
                        else { "A Spiral query is still running. Results update when it finishes." }
                    }
                }
                label { r#for: "workflow-agent-input", class: "sr-only", "Message the Workflow Assistant" }
                div { class: "workflow-composer-field", class: if composer_disabled { "is-disabled" },
                    textarea {
                        id: "workflow-agent-input",
                        rows: "1",
                        maxlength: "8000",
                        disabled: composer_disabled,
                        placeholder: if !state.history_available() { "Assistant history is unavailable" } else if active { "Assistant is working…" } else if configured { "Ask about your robotics data…" } else { "Assistant is temporarily unavailable" },
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
                    if active {
                        Action {
                            purpose: ActionPurpose::Stop,
                            variant: ActionVariant::Outline,
                            class: "ui-action--square workflow-composer-send",
                            disabled: command == Some(AgentPendingCommand::Cancelling),
                            aria_busy: command == Some(AgentPendingCommand::Cancelling),
                            aria_label: if command == Some(AgentPendingCommand::Cancelling) { "Stopping the Assistant turn" } else { "Stop the Assistant turn" },
                            title: "Stop this turn. A submitted Spiral query keeps running.",
                            onclick: move |_| controller.cancel_agent(studio),
                            if command == Some(AgentPendingCommand::Cancelling) { SpinnerIcon {} } else { StopIcon {} }
                        }
                    } else {
                        Action {
                            purpose: ActionPurpose::Start,
                            variant: ActionVariant::Solid,
                            class: "ui-action--square workflow-composer-send",
                            disabled: !can_send,
                            aria_busy: command == Some(AgentPendingCommand::Sending),
                            aria_label: "Send to the Workflow Assistant",
                            title: if !catalog_available { "Reconnect Spiral before sending a message" } else if !configured { "Assistant is temporarily unavailable" } else { "Send · Enter" },
                            onclick: move |_| {
                                if can_send {
                                    let text = drafts.peek().get(&send_key).cloned().unwrap_or_default();
                                    controller.send_agent(text, studio);
                                }
                            },
                            if command == Some(AgentPendingCommand::Sending) { SpinnerIcon {} } else { SendIcon {} }
                        }
                    }
                }
                div { class: "workflow-composer-meta",
                    span {
                        class: if active { "workflow-assistant-presence is-active" } else if configured && !loading { "workflow-assistant-presence is-ready" } else { "workflow-assistant-presence" },
                        role: "status",
                        "aria-live": "polite",
                        "aria-atomic": "true",
                        "{status}"
                    }
                    span { class: "workflow-composer-hint",
                        if loading { "Loading workflow…" } else { "{composer_hint}" }
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
            return "You can stop this turn at any time.";
        }
        if self.activity.command.is_some() {
            return "Finishing the current Assistant action…";
        }
        if self.activity.draft_empty {
            return "Enter to send · Shift+Enter for a new line";
        }
        "Enter to send · Shift+Enter for a new line"
    }
}

#[component]
fn WorkflowSqlEditor(controller: super::state::WorkflowController) -> Element {
    let spec = controller.spec();
    let validation = controller.validation();
    let issues = if spec.sql().trim().is_empty() {
        Vec::new()
    } else {
        validation.errors().to_vec()
    };
    // The coloured copy sits under a transparent textarea with identical
    // metrics; both grow with the text and scroll together in one surface.
    rsx! {
        section { class: "workflow-sql-editor", "aria-label": "SQL definition",
            div { class: "workflow-sql-surface",
                pre { class: "workflow-sql-highlight", "aria-hidden": "true",
                    SqlHighlight { sql: spec.sql().to_owned() }
                    "\n"
                }
                textarea {
                    id: "workflow-sql", class: "workflow-sql-input", spellcheck: "false",
                    "aria-label": "SQL definition",
                    value: spec.sql(), placeholder: "SELECT …",
                    oninput: move |event| controller.set_sql(event.value()),
                }
            }
            if !issues.is_empty() {
                div { class: "workflow-sql-issues", role: "alert",
                    for issue in issues {
                        p { "{issue.message()}" }
                    }
                }
            }
        }
    }
}

#[component]
fn WorkflowCanvas(
    controller: super::state::WorkflowController,
    focus_stage: Signal<Option<String>>,
    on_inspect: EventHandler<WorkflowInspection>,
) -> Element {
    let spec = controller.spec();
    let active = controller.snapshot().phase().active();
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
            if let Some(plan) = controller.plan_for(&expected.0) {
                return (expected, Ok(Some(plan)));
            }
            // Debounce typing, but request an opened document immediately.
            if editing {
                gloo_timers::future::TimeoutFuture::new(350).await;
            }
            let result = controller
                .plan_query(&expected.0)
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
        section { class: "workflow-canvas", "aria-label": "Workflow graph",
            if let Some(error) = controller.load_error() {
                div { class: "inline-error", role: "alert", "{error}" }
            } else if !controller.loaded() {
                div { class: "panel-empty", role: "status",
                    SpinnerIcon {}
                    strong { "Loading workflow…" }
                }
            } else if spec.sql().trim().is_empty() {
                div { class: "panel-empty is-quiet",
                    p { "The graph appears here once the workflow has SQL: one stage per step, reads at the left, the result at the right." }
                }
            } else {
                match current {
                    None => rsx! {
                        div { class: "panel-empty", role: "status",
                            SpinnerIcon {}
                            strong { "Planning SQL…" }
                        }
                    },
                    Some(Err(error)) => rsx! { div { class: "inline-error", role: "alert", "{error}" } },
                    Some(Ok(Some(plan))) => rsx! {
                        if let Some(error) = plan.error() {
                            div { class: "inline-error", role: "alert", "{error}" }
                        }
                        if let Some(report) = plan.report() {
                            super::plan_display::WorkflowPlanDisplay {
                                key: "{spec.id()}:{spec.execution_sql().unwrap_or_default()}", report: report.clone(), active, focus_stage,
                                on_select: move |details| on_inspect.call(WorkflowInspection::new(spec.clone(), details)),
                            }
                        }
                    },
                    Some(Ok(None)) => rsx! {},
                }
            }
        }
    }
}

#[derive(Clone)]
struct WorkflowInspection {
    query: WorkflowPlanInput,
    details: super::plan_display::WorkflowGraphDetails,
}
impl WorkflowInspection {
    fn new(
        workflow: spiral_robotics_common::workflow::WorkflowSpec,
        details: super::plan_display::WorkflowGraphDetails,
    ) -> Self {
        Self {
            query: WorkflowPlanInput(workflow),
            details,
        }
    }
}

/// Document metadata travels with the request, but only identity and SQL
/// invalidate its plan. A save or rename leaves an in-flight request valid.
#[derive(Clone)]
struct WorkflowPlanInput(spiral_robotics_common::workflow::WorkflowSpec);

impl PartialEq for WorkflowPlanInput {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id() && self.0.execution_sql() == other.0.execution_sql()
    }
}

struct RunDuration(u64);

impl RunDuration {
    fn display(&self) -> String {
        if self.0 < 1000 {
            format!("{} ms", self.0)
        } else if self.0 < 60_000 {
            format!("{:.1} s", self.0 as f64 / 1000.0)
        } else {
            let seconds = self.0 / 1000;
            format!("{}m {:02}s", seconds / 60, seconds % 60)
        }
    }
}

#[component]
fn WorkflowRunElapsed(started_ms: u64) -> Element {
    let mut now = use_signal(|| js_sys::Date::now() as u64);
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(1000).await;
            now.set(js_sys::Date::now() as u64);
        }
    });
    let elapsed = now().saturating_sub(started_ms) / 1000;
    rsx! {
        span { class: "run-elapsed", title: "Time since Spiral admitted this run",
            "{elapsed / 60}m {elapsed % 60:02}s"
        }
    }
}

#[component]
fn WorkflowResults(controller: super::state::WorkflowController) -> Element {
    let studio = StudioProjectState::current();
    let snapshot = controller.snapshot();
    let run_reason = controller.run_disabled_reason();

    let spec = controller.spec();
    let runs = controller.workflow_runs();

    let empty_workflow = spec.sql().trim().is_empty();
    let historical_results = runs.is_empty()
        && !empty_workflow
        && controller
            .agent_state()
            .session()
            .messages()
            .iter()
            .any(|message| {
                message
                    .tools()
                    .iter()
                    .any(|tool| tool.name() == "run_workflow")
            });
    let can_run_again = (historical_results
        || matches!(
            snapshot.phase(),
            WorkflowRunPhase::Interrupted | WorkflowRunPhase::Failed
        ))
        && run_reason.is_none();
    let executed = controller.executed_query().unwrap_or_else(|| spec.clone());
    let sql = executed.execution_sql().unwrap_or_else(|error| error);
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
    let (empty_title, empty_detail) = if historical_results {
        (
            "Results from the saved conversation are unavailable",
            "Save keeps SQL and conversation, but not result rows. Run the saved SQL to produce current results; earlier answers describe earlier runs.",
        )
    } else {
        snapshot
            .phase()
            .empty_result_copy(controller.validation().valid(), empty_workflow)
    };
    let selected_position = runs
        .iter()
        .position(|run| run.snapshot().run_id() == snapshot.run_id())
        .map(|index| index + 1)
        .unwrap_or(runs.len());

    rsx! {
        section { class: "workflow-results", "aria-label": "Workflow results",
            if !runs.is_empty() {
                div { class: "run-strip",
                    span { class: "run-phase {snapshot.phase().class_name()}", role: "status",
                        span { class: "run-phase-dot", "aria-hidden": "true" }
                        "{snapshot.phase().run_label()}"
                    }
                    if runs.len() > 1 {
                        label { r#for: "workflow-result-run", class: "sr-only", "Run" }
                        select { id: "workflow-result-run", class: "run-select",
                            value: snapshot.run_id().unwrap_or_default(),
                            onchange: move |event| controller.select_run(event.value()),
                            for (index, run) in runs.iter().enumerate() {
                                option { key: "{run.snapshot().run_id().unwrap_or_default()}",
                                    value: run.snapshot().run_id().unwrap_or_default(),
                                    selected: run.snapshot().run_id() == snapshot.run_id(),
                                    // The phase dot already names the selected run's state; only
                                    // runs that did not complete say so in the list.
                                    if run.snapshot().phase() == WorkflowRunPhase::Completed { "Run {index + 1} of {runs.len()}" }
                                    else { "Run {index + 1} of {runs.len()} · {run.snapshot().phase().run_label().to_ascii_lowercase()}" }
                                }
                            }
                        }
                    } else {
                        span { class: "run-single", "Run {selected_position}" }
                    }
                    if let Some(result) = snapshot.result() {
                        span { class: "run-count",
                            strong { "{result.rows().len()}" }
                            if result.truncated() { " preview rows" } else if result.rows().len() == 1 { " row" } else { " rows" }
                        }
                        span { class: "run-duration", title: "Spiral execution time",
                            "{RunDuration(result.elapsed_ms()).display()}"
                        }
                        if result.truncated() {
                            span { class: "chip chip-caution", title: "Spiral returned more rows than this preview keeps. Use SQL to count or narrow the total.", "Truncated" }
                        }
                    }
                    if snapshot.phase().active() {
                        WorkflowRunElapsed { started_ms: snapshot.started_ms() }
                    }
                    ExecutedSql { sql: sql.clone() }
                }
            }
            if snapshot.result_stale() {
                div { class: "results-stale", role: "status",
                    strong { "Previous run" }
                    span { "{snapshot.phase().stale_result_copy()}" }
                }
            }
            if snapshot.result().is_some() && snapshot.error().is_some() {
                div { class: "inline-error", role: "alert", "{snapshot.error().clone().unwrap_or_default()}" }
            }
            div { key: "{snapshot.run_id().unwrap_or_default()}", class: "workflow-result-body",
                if !controller.loaded() && controller.load_error().is_none() {
                    div { class: "panel-empty", role: "status",
                        SpinnerIcon {}
                        strong { "Loading workflow…" }
                    }
                } else if let Some(model) = result_model.or(transient_model) {
                    {model.render()}
                } else if empty_workflow {
                    div { class: "panel-empty is-quiet",
                        p { "{empty_detail}" }
                    }
                } else {
                    div { class: "panel-empty",
                        ResultNodeIcon {}
                        strong { "{empty_title}" }
                        p { "{empty_detail}" }
                        if can_run_again {
                            Action {
                                purpose: ActionPurpose::Retry,
                                variant: ActionVariant::Outline,
                                onclick: move |_| controller.run(studio),
                                if historical_results { "Run saved SQL" } else { "Run again" }
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
                "Result rows land here after the workflow runs.",
            );
        }
        match self {
            WorkflowRunPhase::Idle if configured => {
                ("No results yet", "Run the workflow to see its rows here.")
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
                "Results appear when the backend publishes the terminal payload.",
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
            WorkflowRunPhase::Validating | WorkflowRunPhase::Running => {
                "These rows belong to earlier SQL. A submitted query is still running."
            }
            _ => {
                "These rows were produced by different SQL. Run the current SQL to refresh the results."
            }
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn run_durations_read_in_the_unit_a_person_would_say() {
        assert_eq!(RunDuration(640).display(), "640 ms");
        assert_eq!(RunDuration(56_500).display(), "56.5 s");
        assert_eq!(RunDuration(125_000).display(), "2m 05s");
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
