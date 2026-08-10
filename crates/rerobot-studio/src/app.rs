//! The studio shell: global session rails around a stable three-pane bench.

use dioxus::prelude::*;

use crate::data_panel::{DataIcon, DataSettingsPanel, DataWorkspace};
use crate::icons::{LayoutReset, PropertiesPanel, Rail, RobotPanelIcon, SignalsPanelIcon};
use crate::panes::{PaneLayout, PaneWorkspace, StudioPane};
use crate::state::Studio;
use crate::timeline::TimelineRail;

const TAILWIND: Asset = asset!("/assets/tailwind.css");
/// The plotting runtime the Signals panel draws with.
///
/// Fetched from a CDN, pinned to an exact version, and checked against the
/// hash of the bytes that version is expected to have — so the studio either
/// gets that exact file or gets nothing, never something else. Nothing else is
/// loaded from the network, so this is the whole of the studio's third-party
/// surface.
///
/// It is the one part of the interface that can therefore be missing: a bench
/// machine with no route out, or a blocked CDN, leaves the plots undrawn. The
/// panel reports that in its own words and keeps printing every joint's angle
/// from the studio's own telemetry — see [`crate::signals`].
const ECHARTS: &str = "https://cdn.jsdelivr.net/npm/echarts@5.6.0/dist/echarts.min.js";
const ECHARTS_INTEGRITY: &str =
    "sha384-pPi0zxBAoDu6+JXW/C68UZLvBUUtU+7zonhif43rqj7pxsGyqyqzcian2Rj37Rss";
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%2337352f'/%3E%3Cpath d='M9 23V13l7-5 7 5v10' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E";

#[component]
pub fn App() -> Element {
    Studio::use_studio();
    crate::state::use_description();

    // The server and browser must produce the same first tree for hydration.
    // Browser-only preferences are applied after the hydrated app mounts.
    let mut rail_open = use_signal(|| false);
    let mut layout = use_signal(PaneLayout::bench);
    let mut surface = use_signal(|| StudioPane::Scene);
    let mut data_open = use_signal(|| false);
    let mut settings_open = use_signal(|| false);
    let mut settings_revision = use_signal(|| 0_u64);
    let studio = crate::state::studio();

    #[cfg(feature = "web")]
    use_effect(move || {
        rail_open.set(stored_bool("rerobot.dataset.expanded.v2", false));
        if let Some(stored) = PaneLayout::stored() {
            layout.set(stored);
        }
    });

    rsx! {
        document::Stylesheet { href: TAILWIND }
        // `crossorigin` is not decoration: without it the browser will not
        // enforce `integrity` on a cross-origin script.
        document::Script {
            src: ECHARTS.to_string(),
            integrity: ECHARTS_INTEGRITY.to_string(),
            crossorigin: "anonymous".to_string(),
            referrerpolicy: "no-referrer".to_string(),
        }
        document::Title { "rerobot studio" }
        document::Link { rel: "icon", href: FAVICON }
        document::Meta { name: "color-scheme", content: "light" }

        div {
            class: "studio-shell",
            tabindex: "-1",
            // The record loop is forty to sixty repetitions in a sitting with
            // one hand on the arm, so it gets keys. Bound on the shell rather
            // than on the document so they only fire while the studio has focus,
            // and ignored while a field is focused so a letter stays a letter.
            onkeydown: move |event| {
                if event.key() == Key::Escape && settings_open() {
                    settings_open.set(false);
                    return;
                }
                if event.key() == Key::Escape && data_open() {
                    data_open.set(false);
                    return;
                }
                if data_open() || settings_open() {
                    return;
                }
                if event.modifiers().ctrl() || event.modifiers().meta() || event.modifiers().alt() {
                    return;
                }
                if !crate::timeline::shortcuts_enabled() {
                    return;
                }
                let recording = studio.recording();
                match event.key() {
                    Key::Character(pressed) if pressed.eq_ignore_ascii_case("r") => {
                        event.prevent_default();
                        match recording {
                            Some(_) => crate::timeline::stop_recording(studio, true),
                            None => crate::timeline::start_recording(studio),
                        }
                    }
                    // Only meaningful mid-take: this is the "that one was bad"
                    // key, pressed while the arm is still moving.
                    Key::Character(pressed)
                        if pressed.eq_ignore_ascii_case("x") && recording.is_some() =>
                    {
                        event.prevent_default();
                        crate::timeline::stop_recording(studio, false);
                    }
                    _ => {}
                }
            },
            Header {
                robots_open: layout.read().contains(StudioPane::Robots),
                properties_open: layout.read().contains(StudioPane::Properties),
                signals_open: layout.read().signals_open(),
                layout_is_default: layout.read().is_default(),
                rail_open: rail_open(),
                data_open: data_open(),
                surface: surface(),
                on_robots: move |_| {
                    layout.write().toggle(StudioPane::Robots);
                    layout.read().store();
                },
                on_signals: move |_| {
                    layout.write().toggle_signals();
                    layout.read().store();
                },
                on_properties: move |_| {
                    layout.write().toggle(StudioPane::Properties);
                    layout.read().store();
                },
                on_rail: move |_| {
                    let next = !rail_open();
                    rail_open.set(next);
                    store_bool("rerobot.dataset.expanded.v2", next);
                },
                on_data: move |_| data_open.toggle(),
                on_reset: move |_| {
                    layout.write().reset();
                    layout.read().store();
                },
                on_surface: move |next| {
                    if !layout.read().contains(next) {
                        layout.write().show(next);
                        layout.read().store();
                    }
                    surface.set(next);
                },
            }
            if data_open() {
                DataWorkspace {
                    on_close: move |_| data_open.set(false),
                    on_settings: move |_| settings_open.set(true),
                    settings_revision,
                }
            }
            if settings_open() {
                DataSettingsPanel {
                    on_close: move |_| settings_open.set(false),
                    on_saved: move |_| *settings_revision.write() += 1,
                }
            }
            div {
                class: "studio-body",
                PaneWorkspace { layout, mobile_pane: surface() }
            }
            // Below the panes rather than inside the viewport column, and
            // outside the surface switcher's reach. A take is a span of time
            // that belongs to the whole session, and a running recording has to
            // stay visible at every width and on every surface — including the
            // narrow layout where only one pane is on screen at a time.
            //
            // Collapsing the rail during a take leaves the transport behind
            // rather than hiding it: whether an arm is being recorded is not a
            // thing a preference gets to switch off.
            TimelineRail {
                compact: !rail_open(),
                on_toggle: move |_| {
                    let next = !rail_open();
                    rail_open.set(next);
                    store_bool("rerobot.dataset.expanded.v2", next);
                }
            }
            NoticeStrip {}
        }
    }
}

#[component]
fn Header(
    robots_open: bool,
    properties_open: bool,
    signals_open: bool,
    layout_is_default: bool,
    rail_open: bool,
    data_open: bool,
    surface: StudioPane,
    on_robots: EventHandler<()>,
    on_signals: EventHandler<()>,
    on_properties: EventHandler<()>,
    on_rail: EventHandler<()>,
    on_data: EventHandler<()>,
    on_reset: EventHandler<()>,
    on_surface: EventHandler<StudioPane>,
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
                    "aria-pressed": robots_open, "aria-label": "Toggle Robots panel", title: "Toggle Robots panel",
                    onclick: move |_| on_robots.call(()), RobotPanelIcon {}
                }
                button {
                    r#type: "button", class: "icon-button data-workspace-toggle",
                    "aria-pressed": data_open,
                    "aria-label": if data_open { "Close Dataset workspace" } else { "Open Dataset workspace" },
                    title: if data_open { "Close Dataset workspace" } else { "Open Dataset workspace" },
                    onclick: move |_| on_data.call(()),
                    DataIcon {}
                    span { "Data" }
                }
                // Beside Robots because that is the column it stacks into: the
                // arms and what they just did are one subject, and the plots
                // are not a fourth surface competing with the bench.
                button {
                    r#type: "button", class: "icon-button desktop-pane-toggle",
                    "aria-pressed": signals_open, "aria-label": "Toggle Signals plots", title: "Toggle Signals plots",
                    onclick: move |_| on_signals.call(()), SignalsPanelIcon {}
                }
                button {
                    r#type: "button", class: "icon-button desktop-pane-toggle",
                    "aria-pressed": properties_open, "aria-label": "Toggle Properties panel", title: "Toggle Properties panel",
                    onclick: move |_| on_properties.call(()), PropertiesPanel {}
                }
                // Not a `desktop-pane-toggle`: the rail is the only region that
                // reports a running take, so the way to collapse it has to exist
                // at every width, including the one where the panes do not.
                button {
                    r#type: "button", class: "icon-button rail-toggle desktop-dataset-toggle",
                    "aria-pressed": rail_open,
                    "aria-expanded": rail_open,
                    "aria-label": if rail_open { "Collapse Dataset" } else { "Open Dataset" },
                    title: if rail_open { "Collapse Dataset" } else { "Open Dataset" },
                    onclick: move |_| on_rail.call(()),
                    Rail {}
                    span { class: "desktop-dataset-label", "Dataset" }
                }
                if !layout_is_default {
                    button {
                        r#type: "button", class: "icon-button layout-reset",
                        "aria-label": "Reset pane widths and visibility", title: "Reset pane widths and visibility",
                        onclick: move |_| on_reset.call(()), LayoutReset {}
                    }
                }
            }
            div { class: "studio-status",
                Warnings {}
                // Only the exceptional environment is worth a coloured tag.
                // Physical hardware is the ordinary case, and a permanent ochre
                // badge announcing the ordinary case is what teaches an operator
                // to stop reading this cluster — so it says the word quietly and
                // spends the tag on simulation and on not knowing.
                if !controls_available {
                    span {
                        class: "chip chip-caution",
                        title: "The environment cannot be verified while telemetry is unavailable or delayed",
                        "Environment unknown"
                    }
                } else if snapshot.simulated {
                    span {
                        class: "chip chip-caution",
                        title: "Commands affect simulated arms only",
                        "Simulation"
                    }
                } else {
                    span {
                        class: "status-environment",
                        title: "Commands can affect physical hardware",
                        "Physical"
                    }
                }
                // A chip that spends every idle minute saying "Stopped" is not
                // reporting anything; the mode only appears once it is running
                // or has faulted.
                if snapshot.mode.error.is_some() || snapshot.mode.active {
                    span {
                        class: if snapshot.mode.error.is_some() { "chip status-mode chip-danger" } else { "chip status-mode chip-active" },
                        title: "Backend-reported command lifecycle",
                        "{mode_state}"
                    }
                }
                // Likewise for torque: an energized arm is news, a released one
                // is the resting state every other signal already implies.
                if !controls_available {
                    span {
                        class: "chip status-hold",
                        title: "Holding state is unknown while telemetry is unavailable or delayed",
                        "Hold unknown"
                    }
                } else if holding > 0 {
                    span {
                        class: "chip status-hold chip-danger",
                        title: "Motor torque is enabled. An arm may resist motion or keep holding after commands stop.",
                        "{holding} holding"
                    }
                }
                if controls_available && loop_hz > 0.0 {
                    span {
                        class: if rate_slow { "control-rate control-rate-slow" } else { "control-rate" },
                        title: "{rate_title}",
                        "aria-label": "Control loop {loop_hz:.0} hertz, target {target_hz:.0} hertz",
                        span { class: "status-dot" }
                        span { class: "control-rate-label", "Control" }
                        // Fixed width and tabular figures: this number changes
                        // twice a second, and a cluster that reflows every time
                        // a digit drops reads as the interface twitching.
                        span { class: "control-rate-value", "{loop_hz:.0}/{target_hz:.0} Hz" }
                    }
                }
                span {
                    class: "connection-status",
                    title: if controls_available { "Receiving recent live state from the studio server" } else if live() { "The event stream is open, but recent telemetry has not arrived; normal controls are paused" } else { "Telemetry is reconnecting; normal controls are paused" },
                    span { class: "status-dot", style: if controls_available { "background: var(--tone-good)" } else { "background: var(--tone-danger)" } }
                    span { class: "connection-label",
                        {if controls_available { "Live".to_owned() } else if live() && telemetry_age.is_finite() { format!("Delayed {telemetry_age:.1}s") } else { "Reconnecting".to_owned() }}
                    }
                }
            }
            nav { class: "mobile-surface-switcher", "aria-label": "Workspace view",
                for choice in StudioPane::ALL {
                    button {
                        r#type: "button",
                        class: if surface == choice { "mobile-surface-active" } else { "" },
                        "aria-pressed": surface == choice,
                        onclick: move |_| on_surface.call(choice),
                        "{choice.label()}"
                    }
                }
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
    // Several sensors can report the same root failure. The operator needs one
    // recovery path, not one raw implementation string per device.
    let mut copies = Vec::<IssueCopy>::new();
    for issue in &showing {
        let copy = issue_copy(issue);
        if !copies.iter().any(|known| known.message == copy.message) {
            copies.push(copy);
        }
    }
    let count = copies.len();
    let first = copies
        .first()
        .map(|copy| copy.message.clone())
        .unwrap_or_default();
    rsx! {
        details { class: "issue-details", role: "status",
            summary { class: "issue-summary",
                span { class: "issue-icon", "!" }
                span { if count == 1 { "1 issue" } else { "{count} issues" } }
                span { class: "issue-preview", "{first}" }
            }
            div { class: "issue-popover",
                ul { class: "issue-list",
                    for copy in copies {
                        li { key: "{copy.message}",
                            strong { "{copy.message}" }
                            if let Some(detail) = copy.detail {
                                details { class: "technical-details",
                                    summary { "Technical details" }
                                    code { "{detail}" }
                                }
                            }
                        }
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IssueCopy {
    message: String,
    detail: Option<String>,
}

fn issue_copy(issue: &str) -> IssueCopy {
    let lowered = issue.to_ascii_lowercase();
    let message = if lowered.contains("camera")
        && (lowered.contains("lockforconfiguration")
            || lowered.contains("lock rejected")
            || lowered.contains("busy"))
    {
        "A camera is busy. Close other camera apps, then rescan.".to_owned()
    } else if lowered.contains("camera") && lowered.contains("unavailable") {
        "A camera is unavailable. Rescan devices or assign another camera.".to_owned()
    } else if lowered.contains("device or resource busy")
        || lowered.contains("serial port") && lowered.contains("use")
    {
        "An arm's serial port is already in use. Close the other connection, then try again."
            .to_owned()
    } else {
        "The studio needs attention. Open Technical details for diagnostics.".to_owned()
    };
    IssueCopy {
        detail: (message != issue).then(|| issue.to_owned()),
        message,
    }
}

/// A notice floats over the shell until it ages out — see [`crate::state::Notice`],
/// which carries its own deadline, and the studio clock, which enforces it.
#[component]
fn NoticeStrip() -> Element {
    let studio = crate::state::studio();
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
                button { r#type: "button", class: "link", onclick: move |_| notice.set(None), "Dismiss" }
            }
        }
    }
}

#[cfg(feature = "web")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

#[cfg(feature = "web")]
fn stored_bool(key: &str, fallback: bool) -> bool {
    storage()
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

#[cfg(feature = "web")]
fn store_bool(key: &str, value: bool) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, &value.to_string());
    }
}

#[cfg(not(feature = "web"))]
fn store_bool(_key: &str, _value: bool) {}
