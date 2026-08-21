//! Low-frequency Studio chrome around the activity workspaces.

use dioxus::prelude::*;
use spiral_robotics_contracts::api::BackendReadinessState;
use spiral_robotics_contracts::workflow::{WorkflowRunPhase, WorkflowStateResponse};
use spiral_robotics_ui::{Action, ActionPurpose, ActionVariant, focus_after_render};

use crate::frontend::app::activity::Activity;
use crate::frontend::app::notifications::MessageAction;
use crate::frontend::components::icons::{
    DataIcon, NotificationIcon, RobotPanelIcon, RolloutIcon, SettingsIcon, TrainingIcon,
    WorkflowIcon,
};

#[component]
pub(crate) fn ActivityRail(
    activity: Activity,
    settings_active: bool,
    on_activity: EventHandler<Activity>,
    on_settings: EventHandler<()>,
) -> Element {
    rsx! {
        nav { class: "studio-activity-rail", "aria-label": "Application",
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if !settings_active && activity == Activity::Live { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: !settings_active && activity == Activity::Live,
                onclick: move |_| on_activity.call(Activity::Live),
                RobotPanelIcon {}
                span { "Live" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if !settings_active && activity == Activity::Data { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: !settings_active && activity == Activity::Data,
                onclick: move |_| on_activity.call(Activity::Data),
                DataIcon {}
                span { "Data" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if !settings_active && activity == Activity::Workflow { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: !settings_active && activity == Activity::Workflow,
                onclick: move |_| on_activity.call(Activity::Workflow),
                WorkflowIcon {}
                span { "Workflow" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if !settings_active && activity == Activity::Training { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: !settings_active && activity == Activity::Training,
                onclick: move |_| on_activity.call(Activity::Training),
                TrainingIcon {}
                span { "Training" }
            }
            Action {
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if !settings_active && activity == Activity::Rollout { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                aria_pressed: !settings_active && activity == Activity::Rollout,
                onclick: move |_| on_activity.call(Activity::Rollout),
                RolloutIcon {}
                span { "Rollout" }
            }
            Action {
                id: "settings-rail-trigger",
                purpose: ActionPurpose::Navigate,
                variant: ActionVariant::Row,
                class: if settings_active { "activity-rail-item activity-rail-item-active activity-rail-settings" } else { "activity-rail-item activity-rail-settings" },
                aria_pressed: settings_active,
                aria_label: "Settings",
                title: "Settings",
                onclick: move |_| on_settings.call(()),
                SettingsIcon {}
                span { "Settings" }
            }
        }
    }
}

#[component]
pub(crate) fn StatusBar(mode: Activity, on_open_episode: EventHandler<String>) -> Element {
    let mut studio = crate::frontend::app::state::studio();
    let snapshot = studio.snapshot.read().clone();
    let telemetry = studio.telemetry.read().clone();
    let training = studio.training.read().clone();
    let workflow = studio.workflow_state.read().clone();
    let connected = studio.controls_available();
    let backend_connected = (studio.live)();
    let (service_label, service_style) = if !backend_connected {
        ("Reconnecting", "background: var(--tone-danger)")
    } else {
        match snapshot.readiness.state {
            BackendReadinessState::Bootstrapping => {
                ("Starting…", "background: var(--tone-caution)")
            }
            BackendReadinessState::Ready
                if !matches!(mode, Activity::Training | Activity::Workflow) && !connected =>
            {
                ("Reconnecting", "background: var(--tone-danger)")
            }
            BackendReadinessState::Ready => ("Connected", "background: var(--tone-good)"),
            BackendReadinessState::Degraded if snapshot.readiness.catalog_available => {
                ("Limited", "background: var(--tone-caution)")
            }
            BackendReadinessState::Degraded => ("Unavailable", "background: var(--tone-danger)"),
        }
    };
    let connected_arms = snapshot
        .robots
        .iter()
        .filter(|robot| robot.connection.connected)
        .count();
    let holding = telemetry.robots.iter().filter(|robot| robot.torque).count();
    let mode_label = snapshot.mode.selection.label();
    let status_mode = format!("{} · {mode_label}", mode.label());
    let arms_label = format!("{connected_arms}/{} arms", snapshot.robots.len());
    // A bench with nothing wrong on it does not need a line saying so, and the
    // count that appears when something is wrong should be able to say "1 robot
    // problem" without the trailing s the fixed string used to force.
    let robot_problems_label = match snapshot.issues.len() {
        0 => None,
        1 => Some("1 robot problem".to_owned()),
        count => Some(format!("{count} robot problems")),
    };
    let training_problem_count = snapshot.issues.len() + usize::from(!training.readiness.ready);
    let training_problems_label = format!(
        "{training_problem_count} {}",
        if training_problem_count == 1 {
            "problem"
        } else {
            "problems"
        }
    );
    let rate_label = format!("{:.0}/{:.0} Hz", telemetry.loop_hz, telemetry.target_hz);
    let recording_label = telemetry.recording.as_ref().map(|recording| {
        format!(
            "Recording {} · {}",
            format_elapsed_status(recording.elapsed_seconds),
            recording.dataset_id,
        )
    });
    let training_run = training.active_run();
    let training_phase = training_run.map(|run| run.phase.label());
    let training_step =
        training_run.map(|run| format!("step {} / {}", run.current_step, run.config.steps));
    let training_runs_label = format!("{} runs", training.runs.len());
    let workflow_status = workflow_status_projection(
        workflow.as_ref(),
        backend_connected,
        snapshot.readiness.state,
        snapshot.readiness.catalog_available,
    );
    let center = studio.notifications.read().clone();
    let now = (studio.clock_ms)();
    let unread = center.unread_count();
    let latest = center.latest_prominent(now).cloned();
    let message_count = center.messages().count();
    let center_open = center.open();
    let saved_to_data = center.messages().rev().find_map(|message| {
        if message.read {
            return None;
        }
        message
            .action
            .as_ref()
            .map(|MessageAction::OpenEpisode(episode)| (message.id, episode.clone()))
    });
    let mut center_was_open = use_signal(|| false);
    use_effect(use_reactive((&center_open,), move |(center_open,)| {
        let was_open = center_was_open();
        if center_open && !was_open {
            focus_after_render("notification-center-close".to_owned());
        } else if !center_open && was_open {
            focus_after_render("notification-center-trigger".to_owned());
        }
        if was_open != center_open {
            center_was_open.set(center_open);
        }
    }));

    rsx! {
        if let Some((message_id, episode)) = saved_to_data {
            SavedToDataCue {
                key: "saved-{message_id}",
                episode,
            }
        }
        if center_open {
            aside {
                id: "notification-center",
                class: "notification-center",
                role: "region",
                "aria-labelledby": "notification-center-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        studio.notifications.write().close();
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
                            onclick: move |_| studio.notifications.write().clear(),
                            "Clear all"
                        }
                    }
                    Action {
                        id: "notification-center-close",
                        purpose: ActionPurpose::Close,
                        variant: ActionVariant::Link,
                        class: "notification-center-action",
                        onclick: move |_| studio.notifications.write().close(),
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
                            li { class: "notification-item {message.severity.class_name()}", key: "{message.id}",
                                div { class: "notification-item-heading",
                                    span { class: "notification-severity", "{message.severity.label()}" }
                                    strong { "{message.source.label()}" }
                                    time { "{format_message_age(now, message.raised_ms)}" }
                                }
                                p { "{message.text}" }
                                div { class: "notification-item-actions",
                                    if let Some(action) = message.action.clone() {
                                        {
                                            let action_label = action.label();
                                            let action_for_click = action.clone();
                                            rsx! { Action {
                                                purpose: ActionPurpose::Navigate,
                                                variant: ActionVariant::Solid,
                                                class: "notification-primary-action",
                                                onclick: move |_| {
                                                    match &action_for_click {
                                                        MessageAction::OpenEpisode(episode) => on_open_episode.call(episode.clone()),
                                                    }
                                                    studio.notifications.write().close();
                                                },
                                                "{action_label}"
                                            } }
                                        }
                                    }
                                    Action {
                                        purpose: ActionPurpose::Remove,
                                        variant: ActionVariant::Link,
                                        class: "notification-center-action",
                                        onclick: move |_| studio.notifications.write().dismiss(message.id),
                                        "Dismiss"
                                    }
                                }
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
                        title: "{snapshot.readiness.detail}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    span { class: "status-bar-item",
                        if training.readiness.ready { "Training ready" } else { "Training unavailable" }
                    }
                    span { class: "status-bar-item", "{training_runs_label}" }
                }
                if let Some(message) = latest.clone() {
                    span {
                        class: "status-bar-message {message.severity.class_name()}",
                        title: "{message.text}",
                        role: if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text}"
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
                        title: "{snapshot.readiness.detail}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    span { class: "status-bar-item", "{workflow_status.domain}" }
                    span { class: "status-bar-item", "{workflow_status.phase}" }
                }
                if let Some(message) = latest.clone() {
                    span {
                        class: "status-bar-message {message.severity.class_name()}",
                        title: "{message.text}",
                        role: if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text}"
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
                        title: "{snapshot.readiness.detail}",
                        span { class: "status-dot", style: service_style }
                        "{service_label}"
                    }
                    span { class: "status-bar-item", "Physical hardware" }
                    span { class: "status-bar-item", "{arms_label}" }
                }
                if let Some(message) = latest {
                    span {
                        class: "status-bar-message {message.severity.class_name()}",
                        title: "{message.text}",
                        role: if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "alert" } else { "status" },
                        "aria-live": if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                        "{message.text}"
                    }
                }
                div { class: "status-bar-group status-bar-group-right",
                    if let Some(recording_label) = recording_label {
                        span { class: "status-bar-item status-bar-danger", "{recording_label}" }
                    }
                    if holding > 0 { span { class: "status-bar-item status-bar-danger", "{holding} holding" } }
                    span { class: "status-bar-item status-bar-accent", "{status_mode}" }
                    if telemetry.target_hz > 0.0 { span { class: "status-bar-item status-bar-rate", "{rate_label}" } }
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
fn SavedToDataCue(episode: String) -> Element {
    rsx! {
        div {
            class: "live-save-confirmation",
            role: "status",
            "aria-live": "polite",
            svg {
                view_box: "0 0 16 16",
                "aria-hidden": "true",
                circle { cx: "8", cy: "8", r: "7" }
                path { d: "m4.5 8 2.2 2.2 4.8-5" }
            }
            span {
                strong { "Saved to Data" }
                small { "Episode {crate::frontend::screens::data::workbench::short_id(&episode)}" }
            }
        }
    }
}

#[component]
fn NotificationTrigger(unread: usize, open: bool) -> Element {
    let mut studio = crate::frontend::app::state::studio();
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
            onclick: move |_| studio.notifications.write().toggle(),
            NotificationIcon {}
            if unread > 0 { span { "{unread}" } }
        }
    }
}

fn format_message_age(now_ms: f64, raised_ms: f64) -> String {
    let seconds = ((now_ms - raised_ms) / 1_000.0).max(0.0);
    if seconds < 60.0 {
        "now".to_owned()
    } else if seconds < 3_600.0 {
        format!("{}m", (seconds / 60.0).floor() as u64)
    } else {
        format!("{}h", (seconds / 3_600.0).floor() as u64)
    }
}

pub(crate) fn format_elapsed_status(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}

#[derive(Debug, Eq, PartialEq)]
struct WorkflowStatusProjection {
    domain: String,
    phase: String,
    graph: String,
}

fn workflow_status_projection(
    state: Option<&WorkflowStateResponse>,
    backend_connected: bool,
    readiness: BackendReadinessState,
    catalog_available: bool,
) -> WorkflowStatusProjection {
    if !backend_connected
        || (readiness != BackendReadinessState::Bootstrapping && !catalog_available)
    {
        return WorkflowStatusProjection {
            domain: "Workflow unavailable".to_owned(),
            phase: "State unavailable".to_owned(),
            graph: "Graph unavailable".to_owned(),
        };
    }

    let Some(state) = state.filter(|_| readiness != BackendReadinessState::Bootstrapping) else {
        return WorkflowStatusProjection {
            domain: "Spiral relations".to_owned(),
            phase: "Loading saved workflow…".to_owned(),
            graph: "Loading graph…".to_owned(),
        };
    };

    let phase = match state.snapshot.phase {
        WorkflowRunPhase::Idle => "Idle · saved",
        WorkflowRunPhase::Validating => "Validating",
        WorkflowRunPhase::Running => "Running",
        WorkflowRunPhase::Cancelling => "Cancelling",
        WorkflowRunPhase::Completed => "Complete",
        WorkflowRunPhase::Failed => "Failed",
        WorkflowRunPhase::Cancelled => "Cancelled",
        WorkflowRunPhase::Interrupted => "Interrupted",
    };
    let node_label = if state.workflow.nodes.len() == 1 {
        "node"
    } else {
        "nodes"
    };
    let connection_label = if state.workflow.edges.len() == 1 {
        "connection"
    } else {
        "connections"
    };
    WorkflowStatusProjection {
        domain: "Spiral relations".to_owned(),
        phase: phase.to_owned(),
        graph: format!(
            "{} {node_label} · {} {connection_label} · r{}",
            state.workflow.nodes.len(),
            state.workflow.edges.len(),
            state.workflow.revision,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_status_projects_authoritative_graph_and_run_phase() {
        let mut state = WorkflowStateResponse::default();
        state.workflow.revision = 7;
        state.snapshot.phase = WorkflowRunPhase::Running;

        let status =
            workflow_status_projection(Some(&state), true, BackendReadinessState::Ready, true);

        assert_eq!(status.domain, "Spiral relations");
        assert_eq!(status.phase, "Running");
        assert_eq!(status.graph, "3 nodes · 2 connections · r7");
    }

    #[test]
    fn workflow_status_does_not_invent_state_while_loading_or_unavailable() {
        let loading =
            workflow_status_projection(None, true, BackendReadinessState::Bootstrapping, false);
        assert_eq!(loading.phase, "Loading saved workflow…");
        assert_eq!(loading.graph, "Loading graph…");

        let unavailable = workflow_status_projection(
            Some(&WorkflowStateResponse::default()),
            false,
            BackendReadinessState::Ready,
            true,
        );
        assert_eq!(unavailable.domain, "Workflow unavailable");
        assert_eq!(unavailable.phase, "State unavailable");
        assert_eq!(unavailable.graph, "Graph unavailable");
    }
}
