//! Low-frequency Studio chrome around the activity workspaces.

use dioxus::prelude::*;

use crate::frontend::app::activity::Activity;
use crate::frontend::app::notifications::MessageAction;
use crate::frontend::components::icons::{
    DataIcon, NotificationIcon, RobotPanelIcon, RolloutIcon, SettingsIcon, TrainingIcon,
};

#[component]
pub(crate) fn ActivityRail(
    activity: Activity,
    on_activity: EventHandler<Activity>,
    on_settings: EventHandler<()>,
) -> Element {
    rsx! {
        nav { class: "studio-activity-rail", "aria-label": "Application",
            button {
                r#type: "button",
                class: if activity == Activity::Live { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Live,
                onclick: move |_| on_activity.call(Activity::Live),
                RobotPanelIcon {}
                span { "Live" }
            }
            button {
                r#type: "button",
                class: if activity == Activity::Data { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Data,
                onclick: move |_| on_activity.call(Activity::Data),
                DataIcon {}
                span { "Data" }
            }
            button {
                r#type: "button",
                class: if activity == Activity::Training { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Training,
                onclick: move |_| on_activity.call(Activity::Training),
                TrainingIcon {}
                span { "Training" }
            }
            button {
                r#type: "button",
                class: if activity == Activity::Rollout { "activity-rail-item activity-rail-item-active" } else { "activity-rail-item" },
                "aria-pressed": activity == Activity::Rollout,
                onclick: move |_| on_activity.call(Activity::Rollout),
                RolloutIcon {}
                span { "Rollout" }
            }
            button {
                id: "settings-rail-trigger",
                r#type: "button",
                class: "activity-rail-item activity-rail-settings",
                "aria-label": "Settings",
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
    let connected = studio.controls_available();
    let backend_connected = (studio.live)();
    let connected_arms = snapshot
        .robots
        .iter()
        .filter(|robot| robot.connection.connected)
        .count();
    let holding = telemetry.robots.iter().filter(|robot| robot.torque).count();
    let mode_label = snapshot.mode.selection.label();
    let status_mode = format!("{} · {mode_label}", mode.label());
    let arms_label = format!("{connected_arms}/{} arms", snapshot.robots.len());
    let robot_problems_label = format!("{} robot problems", snapshot.issues.len());
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
    let center = studio.notifications.read().clone();
    let now = (studio.clock_ms)();
    let unread = center.unread_count();
    let latest = center.latest_prominent(now).cloned();
    let message_count = center.messages().count();
    let center_open = center.open();
    let mut center_was_open = use_signal(|| false);
    use_effect(use_reactive((&center_open,), move |(center_open,)| {
        let was_open = center_was_open();
        if center_open && !was_open {
            spawn(async {
                gloo_timers::future::TimeoutFuture::new(0).await;
                focus_element("notification-center-close");
            });
        } else if !center_open && was_open {
            spawn(async {
                gloo_timers::future::TimeoutFuture::new(0).await;
                focus_element("notification-center-trigger");
            });
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
                        studio.notifications.write().close();
                    }
                },
                header { class: "notification-center-header",
                    div {
                        h2 { id: "notification-center-title", "Notifications" }
                        span { "{message_count} in this session" }
                    }
                    if message_count > 0 {
                        button {
                            r#type: "button",
                            class: "notification-center-action",
                            onclick: move |_| studio.notifications.write().clear(),
                            "Clear all"
                        }
                    }
                    button {
                        id: "notification-center-close",
                        r#type: "button",
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
                                        button {
                                            r#type: "button",
                                            class: "notification-primary-action",
                                            onclick: move |_| {
                                                match &action {
                                                    MessageAction::OpenEpisode(episode) => on_open_episode.call(episode.clone()),
                                                }
                                                studio.notifications.write().close();
                                            },
                                            "{action.label()}"
                                        }
                                    }
                                    button {
                                        r#type: "button",
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
                        span { class: "status-dot", style: if backend_connected { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                        if backend_connected { "Connected" } else { "Reconnecting" }
                    }
                    span { class: "status-bar-item",
                        if training.readiness.ready { "Training ready" } else { "Training unavailable" }
                    }
                    span { class: "status-bar-item", "{training_runs_label}" }
                }
                div { class: "status-bar-group status-bar-group-right",
                    if let Some(message) = latest.clone() {
                        span {
                            class: "status-bar-message {message.severity.class_name()}",
                            role: if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "alert" } else { "status" },
                            "aria-live": if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                            "{message.text}"
                        }
                    }
                    if let Some(phase) = training_phase { span { class: "status-bar-item", "{phase}" } }
                    if let Some(step) = training_step { span { class: "status-bar-item", "{step}" } }
                    span { class: "status-bar-item", "{training_problems_label}" }
                    NotificationTrigger { unread, open: center_open }
                }
            }
        } else {
            footer { class: "studio-status-bar",
                div { class: "status-bar-group",
                    span { class: "status-bar-item",
                        span { class: "status-dot", style: if connected { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                        if connected { "Connected" } else { "Reconnecting" }
                    }
                    span { class: "status-bar-item", if snapshot.simulated { "Motors simulated" } else { "Physical hardware" } }
                    span { class: "status-bar-item", "{arms_label}" }
                }
                div { class: "status-bar-group status-bar-group-right",
                    if let Some(message) = latest {
                        span {
                            class: "status-bar-message {message.severity.class_name()}",
                            role: if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "alert" } else { "status" },
                            "aria-live": if message.severity == crate::frontend::app::notifications::MessageSeverity::Error { "assertive" } else { "polite" },
                            "{message.text}"
                        }
                    }
                    if let Some(recording_label) = recording_label {
                        span { class: "status-bar-item status-bar-danger", "{recording_label}" }
                    }
                    if holding > 0 { span { class: "status-bar-item status-bar-danger", "{holding} holding" } }
                    span { class: "status-bar-item status-bar-accent", "{status_mode}" }
                    if telemetry.target_hz > 0.0 { span { class: "status-bar-item", "{rate_label}" } }
                    span { class: "status-bar-item", "{robot_problems_label}" }
                    NotificationTrigger { unread, open: center_open }
                }
            }
        }
    }
}

#[component]
fn NotificationTrigger(unread: usize, open: bool) -> Element {
    let mut studio = crate::frontend::app::state::studio();
    rsx! {
        button {
            id: "notification-center-trigger",
            r#type: "button",
            class: "status-bar-notifications",
            "aria-label": if unread == 0 { "Open notifications" } else { "Open notifications, {unread} unread" },
            "aria-expanded": open,
            "aria-controls": "notification-center",
            title: if unread == 0 { "Notifications" } else { "{unread} unread notifications" },
            onclick: move |_| studio.notifications.write().toggle(),
            NotificationIcon {}
            if unread > 0 { span { "{unread}" } }
        }
    }
}

#[cfg(feature = "web")]
fn focus_element(id: &str) {
    use wasm_bindgen::JsCast as _;

    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = element.focus();
}

#[cfg(not(feature = "web"))]
fn focus_element(_id: &str) {}

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
