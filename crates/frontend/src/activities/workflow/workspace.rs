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
//! left 24% Assistant; right: Graph over Results (54% of the height).
//! Stage details float over the graph only while a stage is selected.
//! FORM: graph hero with a side console and a results shelf.
//! FINISH: unreviewed and undocumented is unfinished; this build ends with the
//! finish review, the verdict, and DESIGN.md.

use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::workflow::{WorkflowAgentPhase, WorkflowRunPhase};
use spiral_robotics_ui::{
    ActionPurpose, ArrowDownIcon, Button, ButtonVariant, Chip, ChipTone, EmptyState, Notice,
    NoticeTone,
};

use crate::activities::data::query_result::{QueryExecutionPresentation, QueryState};
use crate::activities::workflow::explore::graph::WorkflowView;
use crate::activities::workflow::state::{
    ASSISTANT_PANEL_ID, AgentDraftKey, AgentPendingCommand, CANVAS_PANEL_ID, RESULTS_PANEL_ID,
};
use crate::components::icons::{
    ConversationIcon, ResultNodeIcon, SendIcon, SpinnerIcon, StopIcon, WorkflowIcon,
};
use crate::components::run_strip::{RunChoice, RunDuration, RunStrip};
use crate::shell::state::StudioProjectState;

const WORKFLOW_LAYOUT_SCOPE: &str = "workflow-agent-v14";

#[component]
fn WorkflowPanels(
    controller: super::state::WorkflowController,
    exploration_controller: super::explore::state::ExplorationController,
    view: Signal<WorkflowView>,
) -> Element {
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
            rsx! { WorkflowCanvas { controller } },
        )
        .with_class("workflow-canvas-editor")
        .with_tab_icon(rsx! { WorkflowIcon {} }),
        Panel::new(
            ASSISTANT_PANEL_ID,
            "Assistant",
            "workflow-console",
            rsx! { WorkflowAssistant { controller } },
        )
        .with_class("workflow-assistant-editor")
        .with_tab_icon(rsx! { ConversationIcon {} }),
    ];

    rsx! {
        div { class: "workflow-workspace",
            super::document_controls::WorkflowDocumentControls { controller, exploration_controller, view }
            div { class: "workflow-panels",
                if view() == WorkflowView::Explore {
                    super::explore::graph::ExplorePanels { controller: exploration_controller }
                } else {
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
}

struct WorkflowWorkspaceLayout;

impl WorkflowWorkspaceLayout {
    /// The graph owns the large central tile, results run along the bottom,
    /// and the Assistant occupies a tall column at the left. Stage
    /// details float over the graph on demand rather than holding a tile.
    /// Every tile stays dockable and resizable.
    fn initial() -> PanelLayout {
        // A pipeline is wider than it is tall: two stacked sources and a chain
        // need about 220px of canvas, so the graph row takes 46% of the height
        // and the results shelf keeps five rows of previews.
        let evidence = LayoutNode::split(
            "workflow-evidence-rows",
            SplitAxis::Vertical,
            0.46,
            LayoutNode::tile("workflow-canvas", [CANVAS_PANEL_ID]),
            LayoutNode::tile("workflow-results", [RESULTS_PANEL_ID]),
        );
        PanelLayout::new(LayoutNode::split(
            "workflow-columns",
            SplitAxis::Horizontal,
            0.24,
            LayoutNode::tile("workflow-console", [ASSISTANT_PANEL_ID]),
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
    let configured = state.configured();
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
    // Announced to assistive technology only. On screen the field's
    // placeholder, the send/stop control, and the thread's activity fold
    // already say what the Assistant is doing.
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
                        p { "Describe the rows you need. The Assistant inspects the data, writes and runs the SQL, and refines until the rows answer." }
                        if configured {
                            div { class: "workflow-assistant-examples",
                                Button {
                                    purpose: ActionPurpose::Select,
                                    variant: ButtonVariant::Row,
                                    onclick: {
                                        let draft_key = starter_relations_key.clone();
                                        move |_| {
                                            drafts.write().insert(draft_key.clone(), ("Select all episodes but remove the ones whose top camera is too dim.").to_owned());
                                        }
                                    },
                                    "Remove episodes with dim top-camera footage"
                                }
                                Button {
                                    purpose: ActionPurpose::Select,
                                    variant: ButtonVariant::Row,
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
                if let Some(error) = state.session().error() {
                    div { class: "workflow-assistant-error", role: "alert",
                        strong { "Assistant needs attention" }
                        p { "{error}" }
                        if state.history_available() && !active && configured {
                            Button {
                                purpose: ActionPurpose::Retry,
                                variant: ButtonVariant::Outline,
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
                    Button {
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Icon,
                        class: "ui-button--square",
                        aria_label: "Jump to latest",
                        title: "Jump to latest",
                        onclick: move |_| {
                            follow_latest.set(true);
                            show_latest_action.set(false);
                            let scroll = thread_scroll.peek().to_owned();
                            if let Some(scroll) = scroll {
                                scroll.set_scroll_top(scroll.scroll_height());
                            }
                        },
                        ArrowDownIcon {}
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
                        placeholder: if !state.history_available() { "Assistant history is unavailable" } else if active { "Assistant is working…" } else if !catalog_available { "Connect to Spiral to ask about this project" } else if configured { "Ask about your robotics data…" } else { "Assistant is temporarily unavailable" },
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
                        Button {
                            purpose: ActionPurpose::Stop,
                            variant: ButtonVariant::Outline,
                            class: "ui-button--square workflow-composer-send",
                            disabled: command == Some(AgentPendingCommand::Cancelling),
                            aria_busy: command == Some(AgentPendingCommand::Cancelling),
                            aria_label: if command == Some(AgentPendingCommand::Cancelling) { "Stopping the Assistant turn" } else { "Stop the Assistant turn" },
                            title: "Stop this turn. A submitted Spiral query keeps running.",
                            onclick: move |_| controller.cancel_agent(studio),
                            if command == Some(AgentPendingCommand::Cancelling) { SpinnerIcon {} } else { StopIcon {} }
                        }
                    } else {
                        Button {
                            purpose: ActionPurpose::Start,
                            variant: ButtonVariant::Solid,
                            class: "ui-button--square workflow-composer-send",
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
                span { class: "sr-only", role: "status", "aria-live": "polite", "aria-atomic": "true", "{status}" }
            }
        }
    }
}

#[component]
fn WorkflowCanvas(controller: super::state::WorkflowController) -> Element {
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
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{error}" }
            } else if !controller.loaded() {
                EmptyState { icon: rsx! { SpinnerIcon {} }, title: "Loading workflow…" }
            } else if spec.sql().trim().is_empty() {
                EmptyState {
                    p { "Stages appear here once the workflow has SQL." }
                }
            } else {
                match current {
                    None => rsx! {
                        EmptyState { icon: rsx! { SpinnerIcon {} }, title: "Planning SQL…" }
                    },
                    Some(Err(error)) => rsx! { Notice { tone: NoticeTone::Danger, class: "inline-error", "{error}" } },
                    Some(Ok(Some(plan))) => rsx! {
                        if let Some(error) = plan.error() {
                            Notice { tone: NoticeTone::Danger, class: "inline-error", "{error}" }
                        }
                        if let Some(report) = plan.report() {
                            super::plan_display::WorkflowPlanDisplay {
                                key: "{spec.id()}:{spec.execution_sql().unwrap_or_default()}", report: report.clone(), active,
                            }
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
        self.0.id() == other.0.id() && self.0.execution_sql() == other.0.execution_sql()
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
    let snapshot = controller.snapshot();

    let spec = controller.spec();
    let runs = controller.workflow_runs();
    let run_choices = runs
        .iter()
        .enumerate()
        .map(|(place, run)| {
            RunChoice::new(
                run.snapshot().run_id().unwrap_or_default(),
                (place + 1, runs.len()),
                run.snapshot().phase(),
            )
        })
        .collect::<Vec<_>>();
    let current_run = run_choices
        .iter()
        .find(|choice| Some(choice.id()) == snapshot.run_id())
        .cloned();

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
    let result_model = snapshot
        .result()
        .cloned()
        .map(|result| QueryExecutionPresentation::new(QueryState::Ready(result)));
    let transient_model = if result_model.is_none() {
        match snapshot.phase() {
            WorkflowRunPhase::Running => Some(QueryExecutionPresentation::new(QueryState::Running)),
            WorkflowRunPhase::Failed => snapshot
                .error()
                .map(str::to_owned)
                .map(|error| QueryExecutionPresentation::new(QueryState::Failed(error))),
            _ => None,
        }
    } else {
        None
    };
    // Save keeps SQL and conversation, never rows. The bar's Run is the one
    // way to produce them, so the empty shelf says so in a line and offers
    // no second Run.
    let empty_copy = if historical_results {
        "Rows are not saved with the workflow. Run it to see current results."
    } else {
        snapshot
            .phase()
            .empty_result_copy(controller.validation().valid(), empty_workflow)
    };

    rsx! {
        section { class: "workflow-results", "aria-label": "Workflow results",
            if let Some(current) = current_run {
                RunStrip {
                    id: "workflow-result-run",
                    runs: run_choices,
                    current,
                    on_select: move |id| controller.select_run(id),
                    facts: rsx! {
                        if let Some(result) = snapshot.result() {
                            span { class: "run-count",
                                strong { "{result.rows().len()}" }
                                if result.truncated() { " preview rows" } else if result.rows().len() == 1 { " row" } else { " rows" }
                            }
                            span { class: "run-duration", title: "Spiral execution time",
                                "{RunDuration::millis(result.elapsed_ms())}"
                            }
                            if result.truncated() {
                                Chip { tone: ChipTone::Caution, title: "Spiral returned more rows than this preview keeps. Use SQL to count or narrow the total.", "Truncated" }
                            }
                        }
                        if snapshot.phase().active() {
                            WorkflowRunElapsed { started_ms: snapshot.started_ms() }
                        }
                    },
                }
            }
            if snapshot.result_stale() {
                div { class: "results-stale", role: "status",
                    strong { "Previous run" }
                    span { "{snapshot.phase().stale_result_copy()}" }
                }
            }
            if snapshot.result().is_some() && snapshot.error().is_some() {
                Notice { tone: NoticeTone::Danger, class: "inline-error", "{snapshot.error().clone().unwrap_or_default()}" }
            }
            div { key: "{snapshot.run_id().unwrap_or_default()}", class: "workflow-result-body",
                if !controller.loaded() && controller.load_error().is_none() {
                    EmptyState { icon: rsx! { SpinnerIcon {} }, title: "Loading workflow…" }
                } else if let Some(model) = result_model.or(transient_model) {
                    {model.render()}
                } else {
                    EmptyState {
                        p { "{empty_copy}" }
                    }
                }
            }
        }
    }
}

trait WorkflowRunPresentation {
    /// One faint line for an empty results shelf: what will appear and what
    /// produces it. Never a title, icon, or second Run.
    fn empty_result_copy(self, configured: bool, empty_workflow: bool) -> &'static str;
    fn stale_result_copy(self) -> &'static str;
}

impl WorkflowRunPresentation for WorkflowRunPhase {
    fn empty_result_copy(self, configured: bool, empty_workflow: bool) -> &'static str {
        if empty_workflow {
            return "Rows appear here after the workflow runs.";
        }
        match self {
            WorkflowRunPhase::Idle if configured => "Run the workflow to see its rows here.",
            WorkflowRunPhase::Idle => "Fix the SQL issues, then run the workflow.",
            WorkflowRunPhase::Failed => {
                "The run failed before it produced rows. Run the workflow again."
            }
            WorkflowRunPhase::Completed => "The run completed without publishing rows.",
            WorkflowRunPhase::Running => "Rows appear when the run completes.",
        }
    }

    fn stale_result_copy(self) -> &'static str {
        match self {
            WorkflowRunPhase::Failed => {
                "The latest run failed. These rows remain evidence from the previous completed run."
            }
            WorkflowRunPhase::Running => {
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
}

#[component]
pub(crate) fn WorkflowWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Workflow,
        is_active,
    );
    let controller = super::state::WorkflowController::use_controller();
    // Explore keeps its own document, graph, and selection while the Agent
    // is on screen, so its controller lives beside Workflow's rather than
    // inside the view that draws it — and the status bar, which speaks for
    // whichever view is open, reads both from here.
    let exploration_controller = super::explore::state::ExplorationController::use_controller();
    let view = use_signal(|| WorkflowView::Agent);
    controller.use_loading();
    let status = use_memo(move || match view() {
        WorkflowView::Agent => controller
            .editor_status()
            .map(|status| super::state::WorkflowViewStatus::Agent(Box::new(status))),
        WorkflowView::Explore => Some(super::state::WorkflowViewStatus::Exploration(
            exploration_controller.measure(),
        )),
    });
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Workflow(status), WorkflowPanels { controller, exploration_controller, view } } }
}
