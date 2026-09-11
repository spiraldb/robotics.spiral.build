//! Low-frequency Studio chrome around the activity workspaces.

use crate::activities::workflow::state::WorkflowEditorStatus;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant, BrowserFocus};
use dioxus::prelude::*;
use spiral_robotics_common::status::BackendReadinessState;
use spiral_robotics_common::workflow::WorkflowRunPhase;

use crate::components::icons::{
    DataIcon, NotificationIcon, RobotPanelIcon, RolloutIcon, SettingsIcon, TrainingIcon,
    WorkflowIcon,
};
use crate::format::ClockPresentation;
use crate::shell::activity::Activity;

#[component]
pub(crate) fn ActivityRail(activity: Activity, on_activity: EventHandler<Activity>) -> Element {
    rsx! {
        nav { class: "studio-activity-rail", "aria-label": "Application",
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Live { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: activity == Activity::Live,
                onclick: move |_| on_activity.call(Activity::Live),
                RobotPanelIcon {}
                span { "Live" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Data { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: activity == Activity::Data,
                onclick: move |_| on_activity.call(Activity::Data),
                DataIcon {}
                span { "Data" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Workflow { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: activity == Activity::Workflow,
                onclick: move |_| on_activity.call(Activity::Workflow),
                WorkflowIcon {}
                span { "Workflow" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Training { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: activity == Activity::Training,
                onclick: move |_| on_activity.call(Activity::Training),
                TrainingIcon {}
                span { "Training" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Rollout { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: activity == Activity::Rollout,
                onclick: move |_| on_activity.call(Activity::Rollout),
                RolloutIcon {}
                span { "Rollout" }
            }
            Action {
                id: "settings-rail-trigger",
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if activity == Activity::Settings { "activity-rail-item activity-rail-item-active activity-rail-settings" } else { "activity-rail-item activity-rail-settings" },
                aria_pressed: activity == Activity::Settings,
                aria_label: "Settings",
                title: "Settings",
                onclick: move |_| on_activity.call(Activity::Settings),
                SettingsIcon {}
                span { "Settings" }
            }
        }
    }
}

/// The frame reads status directly from the feature that owns it; no mirrored signals.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum WorkspaceStatus {
    #[default]
    General,
    Hardware(crate::robots::HardwareViewState),
    Training(crate::activities::training::history::TrainingHistory),
    Workflow(Memo<Option<WorkflowEditorStatus>>),
}

/// Workspace content stays mounted; only the active workspace supplies shell chrome.
#[component]
pub(crate) fn WorkspaceFrame(
    children: Element,
    workspace: crate::shell::activity::WorkspaceActivity,
    #[props(default)] status: WorkspaceStatus,
) -> Element {
    let route = crate::shell::routes::StudioRoute::for_activity(workspace.activity());
    rsx! {
        div { class: "studio-route-content", style: "grid-template-rows: auto minmax(0, 1fr)",
            div {
                if let WorkspaceStatus::Hardware(hardware) = status {
                    if let Some(error) = hardware.setup().error() {
                        div { class: "flex items-center gap-2 px-3 py-2 text-xs text-danger", role: "alert",
                            span { "Robot setup unavailable: {error}" }
                            Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Link, onclick: move |_| hardware.setup().retry(), "Retry robot setup" }
                        }
                    } else if !hardware.setup().loaded() {
                        div { class: "px-3 py-2 text-xs text-ink-faint", role: "status", "Loading robot setup…" }
                    }
                }
            }
            {children}
        }
        if workspace.is_active() {
            StatusBar { route, status }
        }
    }
}

#[component]
fn StatusBar(route: crate::shell::routes::StudioRoute, status: WorkspaceStatus) -> Element {
    let mode = route.activity();
    let studio = crate::shell::state::StudioProjectState::current();
    let readiness = crate::shell::project::ProjectSessionSignals::current().readiness();
    let snapshot = match status {
        WorkspaceStatus::Hardware(hardware) => hardware.snapshot().read().clone(),
        _ => Default::default(),
    };
    let (telemetry, connected) = match status {
        WorkspaceStatus::Hardware(robot) => {
            (robot.frame().read().clone(), robot.controls_available())
        }
        _ => (Default::default(), false),
    };
    let training = match status {
        WorkspaceStatus::Training(history) => TrainingStatus::from(&*history.snapshot().read()),
        _ => TrainingStatus::default(),
    };
    let workflow = match status {
        WorkspaceStatus::Workflow(snapshot) => {
            snapshot.read().as_ref().map(WorkflowStatusProjection::from)
        }
        _ => None,
    };
    let backend_connected = (studio.live())();
    let (service_label, service_style) = if !backend_connected {
        ("Reconnecting", "background: var(--tone-danger)")
    } else {
        match readiness.state() {
            BackendReadinessState::Bootstrapping => {
                ("Loading project…", "background: var(--tone-caution)")
            }
            BackendReadinessState::Ready if matches!(status, WorkspaceStatus::Hardware(hardware) if hardware.setup().error().is_some()) => {
                ("Robot setup unavailable", "background: var(--tone-caution)")
            }
            BackendReadinessState::Ready if matches!(status, WorkspaceStatus::Hardware(hardware) if !hardware.setup().available()) => {
                ("Loading robots…", "background: var(--tone-caution)")
            }
            BackendReadinessState::Ready
                if matches!(
                    mode,
                    Activity::Live | Activity::Rollout | Activity::Settings
                ) && !connected =>
            {
                ("Reconnecting", "background: var(--tone-danger)")
            }
            BackendReadinessState::Ready => ("Connected", "background: var(--tone-good)"),
            BackendReadinessState::Degraded if readiness.catalog_available() => {
                ("Limited", "background: var(--tone-caution)")
            }
            BackendReadinessState::Degraded => ("Unavailable", "background: var(--tone-danger)"),
        }
    };
    let connected_arms = snapshot
        .robots()
        .iter()
        .filter(|robot| robot.connection().connected())
        .count();
    let holding = telemetry
        .robots()
        .iter()
        .filter(|robot| robot.torque())
        .count();
    let mode_label = snapshot.mode().selection().label();
    let status_mode = if matches!(status, WorkspaceStatus::Hardware(_)) {
        format!("{} · {mode_label}", route.label())
    } else {
        route.label().to_owned()
    };
    let arms_label = format!("{connected_arms}/{} arms", snapshot.robots().len());
    // A bench with nothing wrong on it does not need a line saying so, and the
    // count that appears when something is wrong should be able to say "1 robot
    // problem" without the trailing s the fixed string used to force.
    let robot_problems_label = match snapshot.issues().len() {
        0 => None,
        1 => Some("1 robot problem".to_owned()),
        count => Some(format!("{count} robot problems")),
    };
    let training_problem_count = usize::from(!training.ready);
    let training_problems_label = format!(
        "{training_problem_count} {}",
        if training_problem_count == 1 {
            "problem"
        } else {
            "problems"
        }
    );
    let rate_label = format!("{:.0}/{:.0} Hz", telemetry.loop_hz(), telemetry.target_hz());
    let recording_label = telemetry.recording().as_ref().map(|recording| {
        format!(
            "Recording {} · {}",
            recording.elapsed_seconds().clock_label(),
            recording.dataset_id(),
        )
    });
    let training_phase = training.phase.as_deref();
    let training_step = training.step.as_deref();
    let training_runs_label = format!("{} runs", training.runs);
    let workflow_status = workflow
        .filter(|_| backend_connected && readiness.catalog_available())
        .unwrap_or_else(|| {
            WorkflowStatusContext {
                state: None,
                backend_connected,
                readiness: readiness.state(),
                catalog_available: readiness.catalog_available(),
            }
            .project()
        });
    let center = studio.notifications().read().to_owned();
    let now = (studio.clock_ms())();
    let unread = center.unread_count();
    let latest = center.latest_prominent(now).cloned();
    let message_count = center.messages().count();
    let center_open = center.open();
    let mut center_was_open = use_signal(|| false);
    use_effect(use_reactive((&center_open,), move |(center_open,)| {
        let was_open = center_was_open();
        if center_open && !was_open {
            BrowserFocus::after_render("notification-center-close".to_owned());
        } else if !center_open && was_open {
            BrowserFocus::after_render("notification-center-trigger".to_owned());
        }
        if was_open != center_open {
            center_was_open.set(center_open);
        }
    }));

    rsx! {
        if center_open {
            aside {
                id: "notification-center",
                class: "notification-center",
                role: "region",
                "aria-labelledby": "notification-center-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        studio.notifications().write().close();
                    }
                },
                header { class: "notification-center-header",
                    div {
                        h2 { id: "notification-center-title", "Notifications" }
                        span { "{message_count} in this session" }
                    }
                    if message_count > 0 {
                        Action {
                            purpose: ActionPurpose::Remove,
                            variant: ActionVariant::Link,
                            class: "notification-center-action",
                            onclick: move |_| studio.notifications().write().clear(),
                            "Clear all"
                        }
                    }
                    Action {
                        id: "notification-center-close",
                        purpose: ActionPurpose::Close,
                        variant: ActionVariant::Link,
                        class: "notification-center-action",
                        onclick: move |_| studio.notifications().write().close(),
                        "Close"
                    }
                }
                if message_count == 0 {
                    div { class: "notification-center-empty",
                        NotificationIcon {}
                        strong { "No notifications" }
                        span { "Command results and recoverable errors will appear here." }
                    }
                } else {
                    ol { class: "notification-list",
                        for message in center.messages().rev().cloned() {
                            {
                                let message_id = message.id();
                                rsx! { li { class: "notification-item {message.severity().class_name()}", key: "{message_id}",
                                div { class: "notification-item-heading",
                                    span { class: "notification-severity", "{message.severity().label()}" }
                                    strong { "{message.source().label()}" }
                                    time { "{message.age_label(now)}" }
                                }
                                p { "{message.text()}" }
                                div { class: "notification-item-actions",
                                    Action {
                                        purpose: ActionPurpose::Remove,
                                        variant: ActionVariant::Link,
                                        class: "notification-center-action",
                                        onclick: move |_| studio.notifications().write().dismiss(message_id),
                                        "Dismiss"
                                    }
                                }
                            } }
                            }
                        }
                    }
                }
            }
        }
        if mode == Activity::Training {
            footer { class: "studio-status-bar",
                div { class: "status-bar-group",
                    span { class: "status-bar-item",
                        title: "{readiness.detail()}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    span { class: "status-bar-item",
                        if training.ready { "Training ready" } else { "Training unavailable" }
                    }
                    span { class: "status-bar-item", "{training_runs_label}" }
                }
                if let Some(message) = latest.clone() {
                    span {
                        class: "status-bar-message {message.severity().class_name()}",
                        title: "{message.text()}",
                        role: if message.severity() == crate::shell::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity() == crate::shell::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text()}"
                    }
                }
                div { class: "status-bar-group status-bar-group-right",
                    if let Some(phase) = training_phase { span { class: "status-bar-item", "{phase}" } }
                    if let Some(step) = training_step { span { class: "status-bar-item", "{step}" } }
                    span { class: "status-bar-item", "{training_problems_label}" }
                    NotificationTrigger { unread, open: center_open }
                }
            }
        } else if mode == Activity::Workflow {
            footer { class: "studio-status-bar",
                div { class: "status-bar-group",
                    span { class: "status-bar-item",
                        title: "{readiness.detail()}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    span { class: "status-bar-item", "{workflow_status.domain}" }
                    span { class: "status-bar-item", "{workflow_status.phase}" }
                }
                if let Some(message) = latest.clone() {
                    span {
                        class: "status-bar-message {message.severity().class_name()}",
                        title: "{message.text()}",
                        role: if message.severity() == crate::shell::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity() == crate::shell::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text()}"
                    }
                }
                div { class: "status-bar-group status-bar-group-right",
                    span { class: "status-bar-item status-bar-accent", "{workflow_status.graph}" }
                    NotificationTrigger { unread, open: center_open }
                }
            }
        } else {
            footer { class: "studio-status-bar",
                div { class: "status-bar-group",
                    span { class: "status-bar-item",
                        title: "{readiness.detail()}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    if matches!(status, WorkspaceStatus::Hardware(_)) {
                        span { class: "status-bar-item", "Physical hardware" }
                        span { class: "status-bar-item", "{arms_label}" }
                    }
                }
                if let Some(message) = latest {
                    span {
                        class: "status-bar-message {message.severity().class_name()}",
                        title: "{message.text()}",
                        role: if message.severity() == crate::shell::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity() == crate::shell::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text()}"
                    }
                }
                div { class: "status-bar-group status-bar-group-right",
                    if let Some(recording_label) = recording_label {
                        span { class: "status-bar-item status-bar-danger", "{recording_label}" }
                    }
                    if holding > 0 { span { class: "status-bar-item status-bar-danger", "{holding} holding" } }
                    span { class: "status-bar-item status-bar-accent", "{status_mode}" }
                    if telemetry.target_hz() > 0.0 { span { class: "status-bar-item status-bar-rate", "{rate_label}" } }
                    if let Some(robot_problems_label) = robot_problems_label {
                        span { class: "status-bar-item status-bar-caution", "{robot_problems_label}" }
                    }
                    NotificationTrigger { unread, open: center_open }
                }
            }
        }
    }
}

#[component]
fn NotificationTrigger(unread: usize, open: bool) -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    rsx! {
        Action {
            id: "notification-center-trigger",
            purpose: ActionPurpose::Toggle,
            variant: ActionVariant::Icon,
            class: "status-bar-notifications",
            aria_label: if unread == 0 { "Open notifications" } else { "Open notifications, {unread} unread" },
            aria_expanded: open,
            aria_controls: "notification-center",
            title: if unread == 0 { "Notifications" } else { "{unread} unread notifications" },
            onclick: move |_| studio.notifications().write().toggle(),
            NotificationIcon {}
            if unread > 0 { span { "{unread}" } }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowStatusProjection {
    domain: String,
    phase: String,
    graph: String,
}

impl From<&WorkflowEditorStatus> for WorkflowStatusProjection {
    fn from(state: &WorkflowEditorStatus) -> Self {
        WorkflowStatusContext {
            state: Some(state),
            backend_connected: true,
            readiness: BackendReadinessState::Ready,
            catalog_available: true,
        }
        .project()
    }
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct TrainingStatus {
    ready: bool,
    runs: usize,
    phase: Option<String>,
    step: Option<String>,
}
impl From<&spiral_robotics_common::training::TrainingSnapshot> for TrainingStatus {
    fn from(snapshot: &spiral_robotics_common::training::TrainingSnapshot) -> Self {
        let run = snapshot.active_run();
        Self {
            ready: snapshot.readiness().ready(),
            runs: snapshot.runs().len(),
            phase: run.map(|run| run.phase().label().to_owned()),
            step: run.map(|run| format!("step {} / {}", run.current_step(), run.config().steps())),
        }
    }
}

struct WorkflowStatusContext<'a> {
    state: Option<&'a WorkflowEditorStatus>,
    backend_connected: bool,
    readiness: BackendReadinessState,
    catalog_available: bool,
}

impl WorkflowStatusContext<'_> {
    fn project(&self) -> WorkflowStatusProjection {
        if !self.backend_connected
            || (self.readiness != BackendReadinessState::Bootstrapping && !self.catalog_available)
        {
            return WorkflowStatusProjection {
                domain: "Workflow unavailable".to_owned(),
                phase: "State unavailable".to_owned(),
                graph: "Graph unavailable".to_owned(),
            };
        }

        let Some(state) = self
            .state
            .filter(|_| self.readiness != BackendReadinessState::Bootstrapping)
        else {
            return WorkflowStatusProjection {
                domain: "Spiral relations".to_owned(),
                phase: "Loading saved workflow…".to_owned(),
                graph: "Loading graph…".to_owned(),
            };
        };

        let phase = match state.snapshot().phase() {
            WorkflowRunPhase::Idle => "Idle",
            WorkflowRunPhase::Validating => "Validating",
            WorkflowRunPhase::Running => "Running",
            WorkflowRunPhase::Cancelling => "Cancelling",
            WorkflowRunPhase::Completed => "Complete",
            WorkflowRunPhase::Failed => "Failed",
            WorkflowRunPhase::Cancelled => "Cancelled",
            WorkflowRunPhase::Interrupted => "Interrupted",
        };
        let node_label = if state.workflow().nodes().len() == 1 {
            "node"
        } else {
            "nodes"
        };
        let connection_label = if state.workflow().edges().len() == 1 {
            "connection"
        } else {
            "connections"
        };
        WorkflowStatusProjection {
            domain: "Spiral relations".to_owned(),
            phase: phase.to_owned(),
            graph: format!(
                "{} {node_label} · {} {connection_label}",
                state.workflow().nodes().len(),
                state.workflow().edges().len(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn workflow_status_projects_authoritative_graph_and_run_phase() {
        let graph = spiral_robotics_common::workflow::WorkflowSpec::default().with_revision(7);
        let mut run = spiral_robotics_common::workflow::WorkflowRunSnapshot::idle_for(&graph);
        run.set_phase(WorkflowRunPhase::Running);
        let state = WorkflowEditorStatus::new(graph, run);

        let status = WorkflowStatusContext {
            state: Some(&state),
            backend_connected: true,
            readiness: BackendReadinessState::Ready,
            catalog_available: true,
        }
        .project();

        assert_eq!(status.domain, "Spiral relations");
        assert_eq!(status.phase, "Running");
        assert_eq!(status.graph, "3 nodes · 2 connections");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn workflow_status_does_not_invent_state_while_loading_or_unavailable() {
        let loading = WorkflowStatusContext {
            state: None,
            backend_connected: true,
            readiness: BackendReadinessState::Bootstrapping,
            catalog_available: false,
        }
        .project();
        assert_eq!(loading.phase, "Loading saved workflow…");
        assert_eq!(loading.graph, "Loading graph…");

        let graph = spiral_robotics_common::workflow::WorkflowSpec::default();
        let state = WorkflowEditorStatus::new(
            graph.clone(),
            spiral_robotics_common::workflow::WorkflowRunSnapshot::idle_for(&graph),
        );
        let unavailable = WorkflowStatusContext {
            state: Some(&state),
            backend_connected: false,
            readiness: BackendReadinessState::Ready,
            catalog_available: true,
        }
        .project();
        assert_eq!(unavailable.domain, "Workflow unavailable");
        assert_eq!(unavailable.phase, "State unavailable");
        assert_eq!(unavailable.graph, "Graph unavailable");
    }
}
