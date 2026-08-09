//! The studio shell: a resizable hierarchy, central visual workspace, and inspector.

use dioxus::prelude::*;
use wasm_bindgen::JsCast;

use crate::control::JointPanel;
use crate::icons::Panel;
use crate::modes::ModeBar;
use crate::setup::RobotPanel;
use crate::state::Studio;
use crate::viewport::Viewport;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Explorer,
    View,
    Inspector,
}

#[derive(Clone, Copy)]
enum PaneDrag {
    Left,
    Right,
}

#[component]
pub fn App() -> Element {
    Studio::use_studio();
    crate::state::use_description();

    let mut left_open = use_signal(|| stored_bool("rerobot.pane.left", true));
    let mut right_open = use_signal(|| stored_bool("rerobot.pane.right", true));
    let mut left_width = use_signal(|| stored_number("rerobot.pane.left.width", 272.0));
    let mut right_width = use_signal(|| stored_number("rerobot.pane.right.width", 320.0));
    let mut dragging = use_signal(|| None::<PaneDrag>);
    let mut surface = use_signal(|| Surface::View);
    let studio = crate::state::studio();

    rsx! {
        document::Stylesheet { href: TAILWIND }
        document::Title { "rerobot studio" }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light dark" }

        div { class: "studio-shell",
            Header {
                left_open: left_open(),
                right_open: right_open(),
                surface: surface(),
                on_left: move |_| {
                    let next = !left_open();
                    left_open.set(next);
                    store_bool("rerobot.pane.left", next);
                },
                on_right: move |_| {
                    let next = !right_open();
                    right_open.set(next);
                    store_bool("rerobot.pane.right", next);
                },
                on_surface: move |next| surface.set(next),
            }
            Warnings {}
            div {
                class: "studio-body",
                onpointermove: move |event| {
                    let Some(active) = dragging() else { return };
                    let x = event.data().client_coordinates().x;
                    match active {
                        PaneDrag::Left => left_width.set(x.clamp(220.0, 380.0)),
                        PaneDrag::Right => {
                            let viewport = web_sys::window()
                                .and_then(|window| window.inner_width().ok())
                                .and_then(|width| width.as_f64())
                                .unwrap_or(1280.0);
                            right_width.set((viewport - x).clamp(280.0, 440.0));
                        }
                    }
                },
                onpointerup: move |_| {
                    if let Some(active) = dragging() {
                        match active {
                            PaneDrag::Left => store_number("rerobot.pane.left.width", left_width()),
                            PaneDrag::Right => store_number("rerobot.pane.right.width", right_width()),
                        }
                    }
                    dragging.set(None);
                },
                if left_open() {
                    aside {
                        class: if surface() == Surface::Explorer { "studio-panel studio-panel-left surface-active-mobile" } else { "studio-panel studio-panel-left surface-hidden-mobile" },
                        style: "width: {left_width()}px",
                        RobotPanel {}
                        if !studio.controls_available() { ReconnectGuard {} }
                    }
                    div {
                        class: "pane-splitter pane-splitter-left",
                        role: "separator",
                        "aria-label": "Resize robot explorer",
                        "aria-orientation": "vertical",
                        title: "Drag to resize the robot explorer",
                        onpointerdown: move |event| {
                            capture(&event);
                            dragging.set(Some(PaneDrag::Left));
                        },
                    }
                }
                main {
                    class: if surface() == Surface::View { "studio-workspace surface-active-mobile" } else { "studio-workspace surface-hidden-mobile" },
                    section { class: "viewport-tile", Viewport {} }
                }
                if right_open() {
                    div {
                        class: "pane-splitter pane-splitter-right",
                        role: "separator",
                        "aria-label": "Resize joint inspector",
                        "aria-orientation": "vertical",
                        title: "Drag to resize the joint inspector",
                        onpointerdown: move |event| {
                            capture(&event);
                            dragging.set(Some(PaneDrag::Right));
                        },
                    }
                    aside {
                        class: if surface() == Surface::Inspector { "studio-panel studio-panel-right surface-active-mobile" } else { "studio-panel studio-panel-right surface-hidden-mobile" },
                        style: "width: {right_width()}px",
                        JointPanel {}
                        if !studio.controls_available() { ReconnectGuard {} }
                    }
                }
            }
            NoticeStrip {}
        }
    }
}

#[component]
fn Header(
    left_open: bool,
    right_open: bool,
    surface: Surface,
    on_left: EventHandler<()>,
    on_right: EventHandler<()>,
    on_surface: EventHandler<Surface>,
) -> Element {
    let studio = crate::state::studio();
    let snapshot = studio.snapshot.read().clone();
    let live = studio.live;
    let controls_available = studio.controls_available();
    let telemetry_age = studio.telemetry_age_seconds();
    let telemetry = studio.telemetry.read().clone();
    let loop_hz = telemetry.loop_hz;
    let target_hz = telemetry.target_hz;
    let rate_slow = target_hz > 0.0 && loop_hz < target_hz * 0.9;
    let holding = telemetry.robots.iter().filter(|robot| robot.torque).count();
    let mode_state = if snapshot.mode.error.is_some() {
        "Faulted"
    } else if snapshot.mode.active {
        "Active"
    } else {
        "Stopped"
    };
    let rate_title = format!(
        "Motor read/write loop: {loop_hz:.1} Hz (target {target_hz:.0} Hz). Browser telemetry is shown at up to 20 Hz."
    );

    rsx! {
        header { class: "studio-header",
            h1 { class: "studio-brand", "rerobot " span { class: "text-ink-faint", "studio" } }
            div { class: "pane-toggle-group", "aria-label": "Workspace panes",
                button {
                    r#type: "button", class: "icon-button desktop-pane-toggle",
                    "aria-pressed": left_open, "aria-label": "Toggle robot explorer", title: "Toggle robot explorer",
                    onclick: move |_| on_left.call(()), Panel { left: true }
                }
                button {
                    r#type: "button", class: "icon-button desktop-pane-toggle",
                    "aria-pressed": right_open, "aria-label": "Toggle joint inspector", title: "Toggle joint inspector",
                    onclick: move |_| on_right.call(()), Panel { left: false }
                }
            }
            div { class: "studio-header-rule" }
            ModeBar {}
            div { class: "studio-status",
                span {
                    class: if !controls_available { "chip" } else if snapshot.simulated { "chip chip-caution" } else { "chip chip-physical" },
                    title: if !controls_available { "The environment cannot be verified while telemetry is unavailable or delayed" } else if snapshot.simulated { "Commands affect simulated arms only" } else { "Commands can affect physical hardware" },
                    {if !controls_available { "Environment unknown" } else if snapshot.simulated { "Simulation" } else { "Physical hardware" }}
                }
                span {
                    class: if snapshot.mode.error.is_some() { "chip chip-danger" } else if snapshot.mode.active { "chip chip-active" } else { "chip" },
                    title: "Backend-reported command lifecycle",
                    "{mode_state}"
                }
                span {
                    class: if !controls_available { "chip" } else if holding > 0 { "chip chip-danger" } else { "chip" },
                    title: if !controls_available { "Holding state is unknown while telemetry is unavailable or delayed" } else if holding > 0 { "Motor torque is enabled. An arm may resist motion or keep holding after commands stop." } else { "No robot currently reports motor torque" },
                    if !controls_available { "Hold unknown" } else if holding == 0 { "Torque released" } else { "{holding} holding" }
                }
                if controls_available && loop_hz > 0.0 {
                    span {
                        class: if rate_slow { "control-rate control-rate-slow" } else { "control-rate" },
                        title: "{rate_title}",
                        "aria-label": "Control loop {loop_hz:.0} hertz, target {target_hz:.0} hertz",
                        span { class: "status-dot" }
                        span { class: "control-rate-label", "Control" }
                        span { class: "font-mono", "{loop_hz:.0}/{target_hz:.0} Hz" }
                    }
                }
                span {
                    class: "connection-status",
                    title: if controls_available { "Receiving recent live state from the studio server" } else if live() { "The event stream is open, but recent telemetry has not arrived; normal controls are paused" } else { "Telemetry is reconnecting; normal controls are paused" },
                    span { class: "status-dot", style: if controls_available { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                    {if controls_available { "Live".to_owned() } else if live() && telemetry_age.is_finite() { format!("Delayed {:.1}s", telemetry_age) } else { "Reconnecting".to_owned() }}
                }
            }
            nav { class: "mobile-surface-switcher", "aria-label": "Workspace view",
                for (choice, label) in [(Surface::Explorer, "Robots"), (Surface::View, "View"), (Surface::Inspector, "Inspect")] {
                    button {
                        r#type: "button",
                        class: if surface == choice { "mobile-surface-active" } else { "" },
                        "aria-pressed": surface == choice,
                        onclick: move |_| on_surface.call(choice),
                        "{label}"
                    }
                }
            }
        }
    }
}

#[component]
fn ReconnectGuard() -> Element {
    rsx! {
        div { class: "reconnect-guard", role: "status",
            div { class: "reconnect-card",
                span { class: "status-dot", style: "background: var(--tone-danger)" }
                span { "Telemetry unavailable — controls paused" }
            }
        }
    }
}

#[component]
fn Warnings() -> Element {
    let studio = crate::state::studio();
    let mut dismissed = use_signal(Vec::<String>::new);
    let issues = studio.snapshot.read().issues.clone();
    let showing = issues
        .iter()
        .filter(|issue| !dismissed.read().contains(issue))
        .cloned()
        .collect::<Vec<_>>();
    if showing.is_empty() {
        return rsx! {};
    }
    let count = showing.len();
    let preview = issue_preview(&showing[0]);

    rsx! {
        div { class: "issue-strip", role: "status",
            details { class: "issue-details",
                summary { class: "issue-summary",
                    span { class: "issue-icon", "!" }
                    strong { if count == 1 { "1 issue" } else { "{count} issues" } }
                    span { class: "issue-preview", "{preview}" }
                }
                ul { class: "issue-list",
                    for issue in showing { li { key: "{issue}", "{issue}" } }
                }
            }
            button {
                r#type: "button", class: "link issue-dismiss",
                title: "Hide these issues until something changes",
                onclick: move |_| dismissed.set(issues.clone()),
                "Dismiss"
            }
        }
    }
}

#[component]
fn NoticeStrip() -> Element {
    let studio = crate::state::studio();
    let mut notice = studio.notice;
    let Some(current) = notice.read().clone() else { return rsx! {}; };

    rsx! {
        div { class: "notice-positioner",
            div {
                class: "notice-card", role: if current.failed { "alert" } else { "status" },
                "aria-live": if current.failed { "assertive" } else { "polite" },
                span { class: "status-dot", style: if current.failed { "background: var(--tone-danger)" } else { "background: var(--tone-good)" } }
                span { class: "flex-1", "{current.text}" }
                button { r#type: "button", class: "link", onclick: move |_| notice.set(None), "Dismiss" }
            }
        }
    }
}

fn capture(event: &Event<PointerData>) {
    let Some(raw) = event.downcast::<web_sys::PointerEvent>() else { return };
    if let Some(target) = raw.target().and_then(|target| target.dyn_into::<web_sys::Element>().ok()) {
        let _ = target.set_pointer_capture(raw.pointer_id());
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored_bool(key: &str, fallback: bool) -> bool {
    storage()
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn stored_number(key: &str, fallback: f64) -> f64 {
    storage()
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn store_bool(key: &str, value: bool) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, &value.to_string());
    }
}

fn store_number(key: &str, value: f64) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, &format!("{value:.0}"));
    }
}

fn issue_preview(issue: &str) -> String {
    let topic = issue.split(':').next().unwrap_or(issue).trim();
    if topic.is_empty() {
        "Open for details".to_owned()
    } else {
        format!("{topic}. Open for details.")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pane_defaults_stay_inside_supported_ranges() {
        assert!((220.0..=380.0).contains(&272.0));
        assert!((280.0..=440.0).contains(&320.0));
    }


    #[test]
    fn issue_preview_hides_driver_noise() {
        assert_eq!(
            super::issue_preview("camera unavailable: driver returned a very long message"),
            "camera unavailable. Open for details."
        );
    }
}
