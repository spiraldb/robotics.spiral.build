//! Low-frequency Studio chrome around the activity workspaces.

use dioxus::prelude::*;

use crate::frontend::app::activity::Activity;
use crate::frontend::components::icons::{
    DataIcon, RobotPanelIcon, RolloutIcon, SettingsIcon, TrainingIcon,
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
pub(crate) fn StatusBar(mode: Activity) -> Element {
    let studio = crate::frontend::app::state::studio();
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
    let problems_label = format!("{} problems", snapshot.issues.len());
    let rate_label = format!("{:.0}/{:.0} Hz", telemetry.loop_hz, telemetry.target_hz);
    let recording_label = telemetry.recording.as_ref().map(|recording| {
        format!(
            "Recording {}",
            format_elapsed_status(recording.elapsed_seconds)
        )
    });
    let training_run = training.active_run();
    let training_phase = training_run.map(|run| run.phase.label());
    let training_step =
        training_run.map(|run| format!("step {} / {}", run.current_step, run.config.steps));
    let training_gpu = training_run
        .and_then(|run| run.metrics.last())
        .and_then(|point| point.gpu_utilization_pct)
        .map(|value| format!("GPU {value:.0}%"));
    let training_cpu = training_run
        .and_then(|run| run.metrics.last())
        .map(|point| format!("CPU {:.0}%", point.cpu_utilization_pct));
    let training_runs_label = format!("{} runs", training.runs.len());

    rsx! {
        if mode == Activity::Training {
            footer { class: "studio-status-bar",
                div { class: "status-bar-group",
                    span { class: "status-bar-item",
                        span { class: "status-dot", style: if backend_connected { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                        if backend_connected { "Backend connected" } else { "Backend reconnecting" }
                    }
                    span { class: "status-bar-item",
                        if training.readiness.ready { "Training runtime ready" } else { "Training runtime unavailable" }
                    }
                    span { class: "status-bar-item", "{training_runs_label}" }
                }
                div { class: "status-bar-group status-bar-group-right",
                    span { class: "status-bar-item status-bar-accent", "Training · ACT" }
                    if let Some(phase) = training_phase { span { class: "status-bar-item", "{phase}" } }
                    if let Some(step) = training_step { span { class: "status-bar-item", "{step}" } }
                    if let Some(gpu) = training_gpu { span { class: "status-bar-item", "{gpu}" } }
                    if let Some(cpu) = training_cpu { span { class: "status-bar-item", "{cpu}" } }
                    span { class: "status-bar-item", "{problems_label}" }
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
                    span { class: "status-bar-item status-bar-accent", "{status_mode}" }
                    if let Some(recording_label) = recording_label {
                        span { class: "status-bar-item status-bar-danger", "{recording_label}" }
                    }
                    if holding > 0 { span { class: "status-bar-item status-bar-danger", "{holding} holding" } }
                    if telemetry.target_hz > 0.0 { span { class: "status-bar-item", "{rate_label}" } }
                    span { class: "status-bar-item", "{problems_label}" }
                }
            }
        }
    }
}

#[component]
pub(crate) fn NoticeStrip(on_view_episode: EventHandler<String>) -> Element {
    let studio = crate::frontend::app::state::studio();
    let mut notice = studio.notice;
    let Some(current) = notice.read().clone() else {
        return rsx! {};
    };

    rsx! {
        div { class: "notice-positioner",
            div {
                class: "notice-card", role: if current.failed { "alert" } else { "status" },
                "aria-live": if current.failed { "assertive" } else { "polite" },
                span { class: "status-dot", style: if current.failed { "background: var(--tone-danger)" } else { "background: var(--tone-good)" } }
                span { class: "flex-1 text-[12px]", "{current.text}" }
                if let Some(episode) = current.view_episode.clone() {
                    button {
                        r#type: "button",
                        class: "btn btn-outline",
                        onclick: move |_| {
                            on_view_episode.call(episode.clone());
                            notice.set(None);
                        },
                        "View in Data"
                    }
                }
                button { r#type: "button", class: "link", onclick: move |_| notice.set(None), "Dismiss" }
            }
        }
    }
}

pub(crate) fn format_elapsed_status(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as u64;
    let remainder = seconds - minutes as f64 * 60.0;
    format!("{minutes}:{remainder:04.1}")
}
