//! Low-frequency Studio chrome around the activity workspaces: the top bar
//! and the messages that float above the workspace.

use crate::activities::curation::state::CurationRuns;
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, BrowserFocus, Button, ButtonVariant, CloseIcon};

use crate::components::icons::{
    CurationIcon, DataIcon, KeyboardIcon, NotificationIcon, RolloutIcon, SettingsIcon, SpiralMark,
    TrainingIcon,
};
use crate::components::page_keys::PageKeys;
use crate::format::ClockPresentation;
use crate::shell::activity::Activity;

/// The one bar across the top of the Studio: the brand, the activities, the
/// open workspace's state (placed here by its frame), and the session's
/// messages and settings at the end.
#[component]
pub(super) fn StudioTopBar(activity: Activity, on_activity: EventHandler<Activity>) -> Element {
    let mut shortcuts_open = use_signal(|| false);
    PageKeys::use_listener(move |event| {
        if !PageKeys::is_shortcut(event) {
            return false;
        }
        let key = event.key();
        if key == "?" {
            shortcuts_open.toggle();
            return true;
        }
        if key == "Escape" && shortcuts_open() {
            shortcuts_open.set(false);
            return true;
        }
        match Activity::from_shortcut(&key) {
            Some(next) => {
                on_activity.call(next);
                true
            }
            None => false,
        }
    });
    rsx! {
        header { class: "studio-topbar-lead",
            div { class: "studio-brand", title: "Spiral Robotics Studio",
                SpiralMark {}
                span { class: "studio-brand-name", "spiral" }
                span { class: "studio-brand-product", "robotics" }
            }
            nav { class: "studio-nav", "aria-label": "Activities",
                for item in Activity::primary() {
                    Button {
                        key: "{item.label()}",
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Tab,
                        class: "studio-nav-item",
                        aria_current: if item == activity { "page" } else { "false" },
                        title: "{item.label()} ({item.shortcut()})",
                        onclick: move |_| on_activity.call(item),
                        ActivityGlyph { activity: item }
                        span { "{item.label()}" }
                    }
                }
            }
        }
        div { class: "studio-topbar-trail",
            Button {
                purpose: ActionPurpose::Toggle,
                variant: ButtonVariant::Icon,
                class: "studio-topbar-action",
                aria_label: "Keyboard shortcuts",
                title: "Keyboard shortcuts (?)",
                aria_expanded: shortcuts_open(),
                onclick: move |_| shortcuts_open.toggle(),
                KeyboardIcon {}
            }
            NotificationTrigger {}
            Button {
                id: "settings-trigger",
                purpose: ActionPurpose::Navigate,
                variant: ButtonVariant::Icon,
                class: "studio-topbar-action",
                aria_label: "Settings",
                aria_pressed: activity == Activity::Settings,
                title: "Settings (,)",
                onclick: move |_| on_activity.call(Activity::Settings),
                SettingsIcon {}
            }
        }
        NotificationCenter {}
        StudioToast {}
        if shortcuts_open() {
            ShortcutSheet { on_close: move |_| shortcuts_open.set(false) }
        }
    }
}

#[component]
fn ActivityGlyph(activity: Activity) -> Element {
    match activity {
        Activity::Data => rsx! { DataIcon {} },
        Activity::Curation => rsx! { CurationIcon {} },
        Activity::Training => rsx! { TrainingIcon {} },
        Activity::Rollout => rsx! { RolloutIcon {} },
        Activity::Settings => rsx! { SettingsIcon {} },
    }
}

/// Every shortcut the Studio answers, grouped by where it works.
#[component]
fn ShortcutSheet(on_close: EventHandler<()>) -> Element {
    let groups: [(&str, Vec<(String, &str)>); 5] = [
        (
            "Anywhere",
            vec![
                ("1 – 4".into(), "Open Data, Curation, Training, Rollout"),
                (",".into(), "Open Settings"),
                ("?".into(), "Show or hide these shortcuts"),
            ],
        ),
        (
            "Data",
            vec![
                ("/".into(), "Filter datasets and episodes"),
                ("J  K".into(), "Next and previous episode"),
                ("Esc".into(), "Back to the dataset"),
            ],
        ),
        (
            "Curation",
            vec![
                ("/".into(), "Filter the node's index"),
                ("J  K".into(), "Next and previous row"),
            ],
        ),
        (
            "Episode playback",
            vec![
                ("Space".into(), "Play or pause"),
                ("←  →".into(), "Step 0.1 s · Shift 1 s · Alt 0.01 s"),
                ("Home  End".into(), "Jump to the start or end"),
                ("+  −  0".into(), "Zoom in, out, or to the whole episode"),
            ],
        ),
        (
            "Rollout",
            vec![
                ("R".into(), "Start or finish recording"),
                ("Shift  X".into(), "Discard the take"),
            ],
        ),
    ];
    rsx! {
        div { class: "ui-dialog-backdrop", onclick: move |_| on_close.call(()) }
        section {
            class: "shortcut-sheet ui-dialog-surface",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "shortcut-sheet-title",
            header {
                h2 { id: "shortcut-sheet-title", "Keyboard shortcuts" }
                Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Icon, aria_label: "Close",
                    onclick: move |_| on_close.call(()), CloseIcon {} }
            }
            div { class: "shortcut-sheet-groups",
                for (title, rows) in groups {
                    section { key: "{title}", class: "shortcut-sheet-group",
                        h3 { "{title}" }
                        dl {
                            for (keys, action) in rows.iter() {
                                div { key: "{keys}",
                                    dt {
                                        for key in keys.split("  ") {
                                            kbd { key: "{key}", class: "ui-kbd", "{key}" }
                                        }
                                    }
                                    dd { "{action}" }
                                }
                            }
                        }
                    }
                }
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
    Curation(Memo<CurationRuns>),
}

/// Workspace content stays mounted; only the active workspace supplies shell chrome.
#[component]
pub(crate) fn WorkspaceFrame(
    children: Element,
    workspace: crate::shell::activity::WorkspaceActivity,
    #[props(default)] status: WorkspaceStatus,
) -> Element {
    rsx! {
        div { class: "studio-route-content", style: "grid-template-rows: auto minmax(0, 1fr)",
            div {
                // A robot connector that is not answering is the status line's
                // to report; until it answers there is no setup to load.
                if let WorkspaceStatus::Hardware(hardware) = status
                    && crate::client::robot_connector::RobotConnector::current().reachable()
                {
                    if let Some(error) = hardware.setup().error() {
                        div { class: "flex items-center gap-2 px-3 py-2 text-xs text-danger", role: "alert",
                            span { "Robot setup unavailable: {error}" }
                            Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Link, onclick: move |_| hardware.setup().retry(), "Retry robot setup" }
                        }
                    } else if !hardware.setup().loaded() {
                        div { class: "px-3 py-2 text-xs text-ink-faint", role: "status", "Loading robot setup…" }
                    }
                }
            }
            {children}
        }
        if workspace.is_active() {
            WorkspaceStatusLine { mode: workspace.activity(), status }
        }
    }
}

/// The open workspace's state in the top bar, most important first: the
/// connection, then the exceptions, then the plain facts. It reads right to
/// left, so when the bar is narrow the least important facts drop whole.
#[component]
fn WorkspaceStatusLine(mode: Activity, status: WorkspaceStatus) -> Element {
    let facts = StatusFacts::of(mode, status);
    let navigator = use_navigator();
    rsx! {
        div { class: "studio-status-line", role: "status",
            crate::shell::workspace::WorkspaceSwitcher {
                tone: facts.connection.tone.name(),
                title: facts.connection.title.clone(),
                span { class: "studio-status-dot" }
                span { class: "studio-status-workspace", "{facts.connection.text}" }
            }
            for (index, fact) in facts.facts.iter().enumerate() {
                match fact.destination.clone() {
                    Some(destination) => rsx! {
                        button { key: "{index}", r#type: "button", class: "studio-status-fact", "data-tone": fact.tone.name(),
                            title: "Open {destination.label()}",
                            onclick: move |_| { navigator.push(destination.clone()); },
                            "{fact.text}"
                        }
                    },
                    None => rsx! {
                        span { key: "{index}", class: "studio-status-fact", "data-tone": fact.tone.name(), "{fact.text}" }
                    },
                }
            }
        }
    }
}

/// How a fact in the status line should read.
#[derive(Clone, Copy, PartialEq)]
enum StatusTone {
    Plain,
    Good,
    Caution,
    Danger,
}

impl StatusTone {
    const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Good => "good",
            Self::Caution => "caution",
            Self::Danger => "danger",
        }
    }
    /// Exceptions come first, so they are the last facts to drop.
    const fn rank(self) -> u8 {
        match self {
            Self::Danger => 0,
            Self::Caution => 1,
            Self::Good | Self::Plain => 2,
        }
    }
}

#[derive(Clone, PartialEq)]
struct StatusFact {
    text: String,
    tone: StatusTone,
    /// Where pressing the fact goes, when it names something to fix.
    destination: Option<crate::shell::routes::StudioRoute>,
}

impl StatusFact {
    fn new(text: impl Into<String>, tone: StatusTone) -> Self {
        Self {
            text: text.into(),
            tone,
            destination: None,
        }
    }
    /// The fact names something to fix in Rollout's Setup.
    fn opening_robot_setup(mut self) -> Self {
        self.destination = Some(crate::shell::routes::StudioRoute::RolloutSetup {});
        self
    }
}

/// The connection the status line leads with: the workspace, once connected,
/// and otherwise what is wrong.
struct StatusConnection {
    text: String,
    title: String,
    tone: StatusTone,
}

/// Everything the status line says, read from the feature that owns it.
struct StatusFacts {
    connection: StatusConnection,
    facts: Vec<StatusFact>,
}

impl StatusFacts {
    fn of(mode: Activity, status: WorkspaceStatus) -> Self {
        let selection = crate::shell::project::ProjectSelection::current();
        let backend_connected = selection.live();
        let project = selection
            .selected()
            .map(|project| project.project_id().to_owned())
            .unwrap_or_default();
        let workspace = match crate::shell::workspace::WorkspaceSettings::current().name() {
            _ if project.is_empty() => "No workspace".to_owned(),
            Some(name) if !name.is_empty() => name,
            _ => project.clone(),
        };
        let hardware = match status {
            WorkspaceStatus::Hardware(hardware) => Some(hardware),
            _ => None,
        };
        let controls_available = hardware.is_some_and(|robot| robot.controls_available());
        let connector = crate::client::robot_connector::RobotConnector::current().health();
        let (text, tone) = if !backend_connected {
            ("Reconnecting".to_owned(), StatusTone::Danger)
        } else if hardware.is_some() && connector.is_none() {
            (
                "Looking for the robot connector…".to_owned(),
                StatusTone::Caution,
            )
        } else if hardware.is_some() && matches!(connector, Some(Err(_))) {
            (
                "Robot connector unreachable".to_owned(),
                StatusTone::Caution,
            )
        } else if hardware.is_some_and(|hardware| hardware.setup().error().is_some()) {
            ("Robot setup unavailable".to_owned(), StatusTone::Caution)
        } else if hardware.is_some_and(|hardware| !hardware.setup().available()) {
            ("Loading robots…".to_owned(), StatusTone::Caution)
        } else if hardware.is_some() && mode == Activity::Rollout && !controls_available {
            ("Reconnecting".to_owned(), StatusTone::Danger)
        } else if project.is_empty() {
            (workspace.clone(), StatusTone::Caution)
        } else {
            (workspace.clone(), StatusTone::Good)
        };
        let connection = StatusConnection {
            title: match tone {
                StatusTone::Good => format!("Workspace {workspace} · Spiral project {project}"),
                _ => text.clone(),
            },
            text,
            tone,
        };
        let mut facts = match status {
            WorkspaceStatus::General => Vec::new(),
            WorkspaceStatus::Hardware(hardware) => Self::hardware(hardware),
            WorkspaceStatus::Training(history) => {
                TrainingStatus::from(&*history.snapshot().read()).facts()
            }
            // A curation has no run of its own; each of its running nodes
            // reports itself, and a curation at rest says nothing.
            WorkspaceStatus::Curation(runs) => runs
                .read()
                .lines()
                .iter()
                .map(|line| StatusFact::new(line.clone(), StatusTone::Plain))
                .collect(),
        };
        facts.sort_by_key(|fact| fact.tone.rank());
        Self { connection, facts }
    }

    fn hardware(hardware: crate::robots::HardwareViewState) -> Vec<StatusFact> {
        let snapshot = hardware.snapshot().read().clone();
        let telemetry = hardware.frame().read().clone();
        let mut facts = Vec::new();
        if let Some(recording) = telemetry.recording().as_ref() {
            facts.push(StatusFact::new(
                format!(
                    "● Rec {} · {}",
                    recording.elapsed_seconds().clock_label(),
                    recording.dataset_id()
                ),
                StatusTone::Danger,
            ));
        }
        let holding = telemetry
            .robots()
            .iter()
            .filter(|robot| robot.torque())
            .count();
        if holding > 0 {
            facts.push(StatusFact::new(
                format!("{holding} holding"),
                StatusTone::Danger,
            ));
        }
        // Arms and the control loop speak only when they fall short; every arm
        // connected at the target rate is the resting state.
        let arms = snapshot.robots().len();
        let connected_arms = snapshot
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .count();
        if connected_arms < arms {
            facts.push(StatusFact::new(
                format!("{connected_arms} of {arms} arms connected"),
                StatusTone::Caution,
            ));
        }
        if telemetry.target_hz() > 0.0 && telemetry.loop_hz() < telemetry.target_hz() * 0.9 {
            facts.push(StatusFact::new(
                format!(
                    "{:.0} of {:.0} Hz",
                    telemetry.loop_hz(),
                    telemetry.target_hz()
                ),
                StatusTone::Caution,
            ));
        }
        match snapshot.issues().len() {
            0 => {}
            1 => facts.push(
                StatusFact::new("1 robot problem", StatusTone::Caution).opening_robot_setup(),
            ),
            count => facts.push(
                StatusFact::new(format!("{count} robot problems"), StatusTone::Caution)
                    .opening_robot_setup(),
            ),
        }
        facts
    }
}

/// The bell: how many messages arrived since the list was last opened.
#[component]
fn NotificationTrigger() -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let center = studio.notifications().read().to_owned();
    let unread = center.unread_count();
    let open = center.open();
    rsx! {
        Button {
            id: "notification-center-trigger",
            purpose: ActionPurpose::Toggle,
            variant: ButtonVariant::Icon,
            class: "studio-topbar-action studio-notifications",
            aria_label: if unread == 0 { "Open notifications" } else { "Open notifications, {unread} unread" },
            aria_expanded: open,
            aria_controls: "notification-center",
            title: if unread == 0 { "Notifications" } else { "{unread} unread notifications" },
            onclick: move |_| studio.notifications().write().toggle(),
            NotificationIcon {}
            if unread > 0 { span { class: "studio-notifications-count", "{unread}" } }
        }
    }
}

/// The session's messages, opened from the bell.
#[component]
fn NotificationCenter() -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let center = studio.notifications().read().to_owned();
    let now = (studio.clock_ms())();
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
    if !center_open {
        return rsx! {};
    }
    rsx! {
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
                    if message_count > 0 {
                        span { "{message_count} in this session" }
                    }
                }
                if message_count > 0 {
                    Button {
                        purpose: ActionPurpose::Remove,
                        variant: ButtonVariant::Link,
                        class: "notification-center-action",
                        onclick: move |_| studio.notifications().write().clear(),
                        "Clear all"
                    }
                }
                Button {
                    id: "notification-center-close",
                    purpose: ActionPurpose::Close,
                    variant: ButtonVariant::Icon,
                    aria_label: "Close notifications",
                    onclick: move |_| studio.notifications().write().close(),
                    CloseIcon {}
                }
            }
            if message_count == 0 {
                div { class: "notification-center-empty",
                    NotificationIcon {}
                    strong { "No notifications" }
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
                                    Button {
                                        purpose: ActionPurpose::Remove,
                                        variant: ButtonVariant::Link,
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
}

/// The newest message, floated above the workspace while it is news. A
/// click opens the full list.
///
/// One toast, updated in place: a newer message replaces the words of the one
/// on screen, because a live region inserted afresh is not reliably announced.
#[component]
fn StudioToast() -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let center = studio.notifications().read().to_owned();
    let now = (studio.clock_ms())();
    let Some(message) = center
        .latest_prominent(now)
        .cloned()
        .filter(|_| !center.open())
    else {
        return rsx! {};
    };
    let severity = message.severity();
    let urgent = severity == crate::shell::notifications::MessageSeverity::Error;
    rsx! {
        button {
            r#type: "button",
            class: "studio-toast {severity.class_name()}",
            role: if urgent { "alert" } else { "status" },
            "aria-live": if urgent { "assertive" } else { "polite" },
            title: "Open notifications",
            onclick: move |_| studio.notifications().write().toggle(),
            span { class: "studio-toast-source", "{message.source().label()}" }
            span { class: "studio-toast-text", "{message.text()}" }
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct TrainingStatus {
    phase: Option<String>,
    step: Option<String>,
}
impl From<&spiral_robotics_common::training::TrainingSnapshot> for TrainingStatus {
    fn from(snapshot: &spiral_robotics_common::training::TrainingSnapshot) -> Self {
        let run = snapshot.active_run();
        Self {
            phase: run.map(|run| run.phase().label().to_owned()),
            step: run.map(|run| format!("step {} / {}", run.current_step(), run.config().steps())),
        }
    }
}

impl TrainingStatus {
    /// An active run speaks for itself; a history at rest says nothing. Why
    /// training is unavailable is the Training panel's to explain, in full.
    fn facts(self) -> Vec<StatusFact> {
        self.phase
            .into_iter()
            .chain(self.step)
            .map(|fact| StatusFact::new(fact, StatusTone::Plain))
            .collect()
    }
}
