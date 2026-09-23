//! Low-frequency Studio chrome around the activity workspaces: the top bar
//! and the messages that float above the workspace.

use crate::activities::curation::state::CurationEditorStatus;
use crate::activities::workflow::state::WorkflowViewStatus;
use dioxus::prelude::*;
use spiral_robotics_common::workflow::WorkflowRunPhase;
use spiral_robotics_ui::{ActionPurpose, BrowserFocus, Button, ButtonVariant, CloseIcon};

use crate::components::icons::{
    CurationIcon, DataIcon, KeyboardIcon, NotificationIcon, RobotPanelIcon, RolloutIcon,
    SettingsIcon, SpiralMark, TrainingIcon, WorkflowIcon,
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
        Activity::Live => rsx! { RobotPanelIcon {} },
        Activity::Data => rsx! { DataIcon {} },
        Activity::Workflow => rsx! { WorkflowIcon {} },
        Activity::Curation => rsx! { CurationIcon {} },
        Activity::Training => rsx! { TrainingIcon {} },
        Activity::Rollout => rsx! { RolloutIcon {} },
        Activity::Settings => rsx! { SettingsIcon {} },
    }
}

/// Every shortcut the Studio answers, grouped by where it works.
#[component]
fn ShortcutSheet(on_close: EventHandler<()>) -> Element {
    let command = PageKeys::command_symbol().trim();
    let groups: [(&str, Vec<(String, &str)>); 5] = [
        (
            "Anywhere",
            vec![
                (
                    "1 – 6".into(),
                    "Open Data, Workflow, Curation, Training, Rollout, Live",
                ),
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
            "Episode playback",
            vec![
                ("Space".into(), "Play or pause"),
                ("←  →".into(), "Step 0.1 s · Shift 1 s · Alt 0.01 s"),
                ("Home  End".into(), "Jump to the start or end"),
                ("+  −  0".into(), "Zoom in, out, or to the whole episode"),
            ],
        ),
        (
            "Workflow and Curation",
            vec![(format!("{command}  ↵"), "Run the graph or the recipe")],
        ),
        (
            "Live",
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
    Workflow(Memo<Option<WorkflowViewStatus>>),
    Curation(Memo<Option<CurationEditorStatus>>),
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
                if let WorkspaceStatus::Hardware(hardware) = status {
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
            span { class: "studio-status-connection", "data-tone": facts.connection.tone.name(),
                title: "{facts.connection.title}",
                span { class: "studio-status-dot" }
                "{facts.connection.text}"
            }
            for (index, fact) in facts.facts.iter().enumerate() {
                match fact.destination {
                    Some(destination) => rsx! {
                        button { key: "{index}", r#type: "button", class: "studio-status-fact", "data-tone": fact.tone.name(),
                            title: "Open {destination.label()}",
                            onclick: move |_| { navigator.push(crate::shell::routes::StudioRoute::for_activity(destination)); },
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
    destination: Option<Activity>,
}

impl StatusFact {
    fn new(text: impl Into<String>, tone: StatusTone) -> Self {
        Self {
            text: text.into(),
            tone,
            destination: None,
        }
    }
    fn opening(mut self, destination: Activity) -> Self {
        self.destination = Some(destination);
        self
    }
}

/// The connection the status line leads with: what is connected, and how.
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
        let hardware = match status {
            WorkspaceStatus::Hardware(hardware) => Some(hardware),
            _ => None,
        };
        let controls_available = hardware.is_some_and(|robot| robot.controls_available());
        let (text, tone) = if !backend_connected {
            ("Reconnecting".to_owned(), StatusTone::Danger)
        } else if hardware.is_some_and(|hardware| hardware.setup().error().is_some()) {
            ("Robot setup unavailable".to_owned(), StatusTone::Caution)
        } else if hardware.is_some_and(|hardware| !hardware.setup().available()) {
            ("Loading robots…".to_owned(), StatusTone::Caution)
        } else if matches!(
            mode,
            Activity::Live | Activity::Rollout | Activity::Settings
        ) && !controls_available
        {
            ("Reconnecting".to_owned(), StatusTone::Danger)
        } else {
            (project.clone(), StatusTone::Good)
        };
        let connection = StatusConnection {
            title: if tone == StatusTone::Good {
                format!("Connected to Spiral project {project}")
            } else {
                text.clone()
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
            WorkspaceStatus::Workflow(snapshot) => snapshot
                .read()
                .as_ref()
                .filter(|_| backend_connected)
                .map(WorkflowStatusProjection::from)
                .unwrap_or_else(|| {
                    WorkflowStatusContext {
                        state: None,
                        backend_connected,
                    }
                    .project()
                })
                .headline()
                .map(|phase| StatusFact::new(phase, StatusTone::Plain))
                .into_iter()
                .collect(),
            WorkspaceStatus::Curation(snapshot) => snapshot
                .read()
                .as_ref()
                .and_then(CurationEditorStatus::headline)
                .map(|headline| StatusFact::new(headline, StatusTone::Plain))
                .into_iter()
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
        let connected_arms = snapshot
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .count();
        facts.push(StatusFact::new(
            format!("{connected_arms}/{} arms", snapshot.robots().len()),
            StatusTone::Plain,
        ));
        if telemetry.target_hz() > 0.0 {
            facts.push(StatusFact::new(
                format!("{:.0}/{:.0} Hz", telemetry.loop_hz(), telemetry.target_hz()),
                StatusTone::Plain,
            ));
        }
        match snapshot.issues().len() {
            0 => {}
            1 => facts.push(
                StatusFact::new("1 robot problem", StatusTone::Caution).opening(Activity::Settings),
            ),
            count => facts.push(
                StatusFact::new(format!("{count} robot problems"), StatusTone::Caution)
                    .opening(Activity::Settings),
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
                    span { "{message_count} in this session" }
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
            key: "{message.id()}",
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

/// What the top bar says about the open workflow view: its phase while it is
/// news, and nothing for an idle graph or an exploration that has measured
/// nothing yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowStatusProjection {
    phase: Option<String>,
}

impl WorkflowStatusProjection {
    fn headline(self) -> Option<String> {
        self.phase
    }
}

impl From<&WorkflowViewStatus> for WorkflowStatusProjection {
    fn from(state: &WorkflowViewStatus) -> Self {
        WorkflowStatusContext {
            state: Some(state),
            backend_connected: true,
        }
        .project()
    }
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct TrainingStatus {
    runs: usize,
    phase: Option<String>,
    step: Option<String>,
}
impl From<&spiral_robotics_common::training::TrainingSnapshot> for TrainingStatus {
    fn from(snapshot: &spiral_robotics_common::training::TrainingSnapshot) -> Self {
        let run = snapshot.active_run();
        Self {
            runs: snapshot.runs().len(),
            phase: run.map(|run| run.phase().label().to_owned()),
            step: run.map(|run| format!("step {} / {}", run.current_step(), run.config().steps())),
        }
    }
}

impl TrainingStatus {
    /// An active run speaks for itself; otherwise the history. Why training
    /// is unavailable is the Training panel's to explain, in full.
    fn facts(self) -> Vec<StatusFact> {
        let mut facts = Vec::new();
        facts.extend(
            self.phase
                .map(|phase| StatusFact::new(phase, StatusTone::Plain)),
        );
        facts.extend(
            self.step
                .map(|step| StatusFact::new(step, StatusTone::Plain)),
        );
        let runs = match self.runs {
            1 => "1 run".to_owned(),
            count => format!("{count} runs"),
        };
        facts.push(StatusFact::new(runs, StatusTone::Plain));
        facts
    }
}

struct WorkflowStatusContext<'a> {
    state: Option<&'a WorkflowViewStatus>,
    backend_connected: bool,
}

impl WorkflowStatusContext<'_> {
    fn project(&self) -> WorkflowStatusProjection {
        let phase = if !self.backend_connected {
            Some("Workflow unavailable".to_owned())
        } else {
            match self.state {
                None => Some("Loading saved workflow…".to_owned()),
                // Explore measures a dataset and has no run of its own.
                Some(WorkflowViewStatus::Exploration(measure)) => {
                    measure.has_measured().then(|| measure.measured())
                }
                Some(WorkflowViewStatus::Agent(agent)) => match agent.snapshot().phase() {
                    WorkflowRunPhase::Idle => None,
                    WorkflowRunPhase::Running => Some("Running".to_owned()),
                    WorkflowRunPhase::Completed => Some("Complete".to_owned()),
                    WorkflowRunPhase::Failed => Some("Failed".to_owned()),
                },
            }
        };
        WorkflowStatusProjection { phase }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::workflow::state::WorkflowEditorStatus;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn workflow_status_projects_the_authoritative_run_phase() {
        let mut run = spiral_robotics_common::workflow::WorkflowRunSnapshot::default();
        run.set_phase(WorkflowRunPhase::Running);
        let state = WorkflowViewStatus::Agent(Box::new(WorkflowEditorStatus::new(run)));

        let status = WorkflowStatusContext {
            state: Some(&state),
            backend_connected: true,
        }
        .project();

        assert_eq!(status.phase.as_deref(), Some("Running"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn workflow_status_does_not_invent_state_while_loading_or_unavailable() {
        let loading = WorkflowStatusContext {
            state: None,
            backend_connected: true,
        }
        .project();
        assert_eq!(loading.phase.as_deref(), Some("Loading saved workflow…"));

        let state = WorkflowViewStatus::Agent(Box::new(WorkflowEditorStatus::new(
            spiral_robotics_common::workflow::WorkflowRunSnapshot::default(),
        )));
        let unavailable = WorkflowStatusContext {
            state: Some(&state),
            backend_connected: false,
        }
        .project();
        assert_eq!(unavailable.phase.as_deref(), Some("Workflow unavailable"));
    }
}
