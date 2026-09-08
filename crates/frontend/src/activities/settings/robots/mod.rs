//! Creating robot instances and getting them ready to move.
//!
//! One arm is one robot instance in SpiralDB. Everything on this panel—the
//! port, cameras, motors, and calibration—is one durable configuration row.
//!
//! The panel lives in Settings as the durable owner of ports, camera
//! assignments, and calibration. Live consumes that configuration without
//! mixing setup work into the operating surface.

use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, ChevronIcon, FileAction, FileInput, UnassignIcon,
};
use dioxus::prelude::*;
use spiral_robotics_common::robots::{
    CameraBinding, NewRobot, PortSettings, RobotSummary, RobotUpdate, SerialPortSummary,
};

use crate::panels::scene::ScenePalette;
use crate::shell::project::ProjectSessionSignals;
use crate::shell::robot_workspace::{RobotWorkspace, Selection};
use crate::tasks::SingleFlight;

mod calibration;
mod delete;
use calibration::CalibrationSection;
use delete::DeleteRobotAction;

/// Robot types offered when creating an instance.
const ROBOT_TYPES: [(&str, &str); 2] = [
    ("so101_follower", "Follower (driven arm)"),
    ("so101_leader", "Leader (hand-guided arm)"),
];

#[component]
pub(super) fn RobotPanel() -> Element {
    let studio = RobotWorkspace::current();
    let robots = studio.snapshot().read().robots().to_owned();
    let focused = studio.focused();
    let sessions = ProjectSessionSignals::current();
    let mut ports = use_resource(move || {
        let available = sessions.catalog_available();
        async move {
            if !available {
                return std::future::pending().await;
            }
            RobotClient::current()
                .list_serial_ports()
                .await
                .map_err(|error| error.to_string())
        }
    });
    let port_catalog =
        PortCatalogState::new(ports.read_unchecked().to_owned(), move |()| ports.restart());

    rsx! {
        div { class: "flex flex-col",
            ul { class: "flex flex-col",
                for (index, robot) in robots.iter().cloned().enumerate() {
                    RobotEntry {
                        key: "{robot.id()}",
                        state: RobotEntryState::new(
                            focused.as_ref().is_some_and(|chosen| chosen.id() == robot.id()),
                            ScenePalette::accent_css(index),
                            port_catalog.clone(),
                        ),
                        robot,
                    }
                }
            }

            div { class: "device-list-actions",
                NewRobotForm {}
                ImportRobotConfiguration {}
            }
        }
    }
}

/// One arm: a row that always shows its state, and its setup underneath.
///
/// The setup is folded away rather than confined to whichever arm is selected,
/// so a two-arm bench can have both open at once — which is exactly when it is
/// needed, since setting up the second arm means comparing it to the first.
#[component]
fn RobotEntry(robot: RobotSummary, state: RobotEntryState) -> Element {
    let RobotEntryState {
        focused,
        accent,
        port_catalog,
    } = state;
    let studio = RobotWorkspace::current();
    let mut chosen = studio.selection();
    // Open on whatever still needs attention. An arm that is wired up and
    // calibrated has nothing to do here, so it starts folded; anything else
    // opens on the work. Only the initial state is derived — an arm becoming
    // ready while the operator is reading it must not fold itself away.
    let ready = robot.problem().is_none() && robot.port().is_some() && robot.calibrated();
    let mut open = use_signal(|| !ready || !robot.connection().connected());
    let id = robot.id().to_owned();
    let role = robot.robot_type().robot_role();
    let state = if robot.problem().is_some() {
        "Needs attention"
    } else if robot.port().is_none() {
        "Port needed"
    } else if !robot.calibrated() {
        "Calibration needed"
    } else if robot.connection().connected() {
        "Connected"
    } else {
        "Ready to connect"
    };

    rsx! {
        li { class: "device-entry",
            div {
                class: "device-entry-row",
                Action {
                    purpose: ActionPurpose::Toggle,
                    variant: ActionVariant::Icon,
                    class: "device-chevron",
                    aria_expanded: open(),
                    aria_label: if open() { "Hide setup for {robot.id()}" } else { "Show setup for {robot.id()}" },
                    title: if open() { "Hide this arm's setup" } else { "Show this arm's setup" },
                    onclick: move |_| open.toggle(),
                    ChevronIcon { open: open() }
                }
                Action {
                    purpose: ActionPurpose::Select,
                    variant: ActionVariant::Row,
                    class: "device-main",
                    aria_pressed: focused,
                    title: "Show {robot.id()} in the joints panel",
                    onclick: move |_| chosen.set(Some(Selection::Robot(id.clone()))),
                    span {
                        class: "device-accent",
                        style: "background: {accent}",
                    }
                    // Name and role share a baseline rather than stacking. The
                    // stack is what made this row 48px tall, which put the
                    // setup underneath it — the part with work in it — below
                    // the fold on a two-arm bench.
                    span { class: "device-copy",
                        strong { "{robot.id()}" }
                        span { "{role}" }
                    }
                    span { class: if ready { "device-state" } else { "device-state device-state-needed" }, "{state}" }
                    span {
                        class: "device-connection-dot",
                        style: if robot.connection().connected() { "background: var(--tone-good)" } else { "background: var(--tone-line)" },
                        title: if robot.connection().connected() { "Connected" } else { "Not connected" },
                    }
                }
            }
            if open() {
                RobotSetup { robot: robot.clone(), port_catalog }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct RobotEntryState {
    focused: bool,
    accent: &'static str,
    port_catalog: PortCatalogState,
}

impl RobotEntryState {
    const fn new(focused: bool, accent: &'static str, port_catalog: PortCatalogState) -> Self {
        Self {
            focused,
            accent,
            port_catalog,
        }
    }
}

/// Adding an arm, kept folded away so the list stays a list.
#[component]
fn NewRobotForm() -> Element {
    let studio = RobotWorkspace::current();
    let mut chosen = studio.selection();
    let mut open = use_signal(|| false);
    let mut id = use_signal(String::new);
    let mut robot_type = use_signal(|| ROBOT_TYPES[0].0.to_owned());
    let creating = use_signal(|| false);

    let create = move || {
        if creating.peek().to_owned() {
            return;
        }
        let name = id.read().trim().to_owned();
        if name.is_empty() {
            studio.report("Give the arm a name first.");
            return;
        }
        let request = NewRobot::new(name.clone(), Some(robot_type()));
        let selected_name = name.clone();
        studio.run_utility(format!("Created {name}."), async move {
            let Some(result) = SingleFlight::run(creating, async move {
                RobotClient::current()
                    .create_robot(request)
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
            else {
                return Ok(());
            };
            result?;
            // Only clear and close after the server accepts the new robot. A
            // rejected name or configuration stays in place for correction.
            id.set(String::new());
            open.set(false);
            chosen.set(Some(Selection::Robot(selected_name)));
            Ok(())
        });
    };

    if !open() {
        return rsx! {
            Action {
                purpose: ActionPurpose::Create,
                variant: ActionVariant::Row,
                class: "device-list-action",
                onclick: move |_| open.set(true),
                "+ Add an arm"
            }
        };
    }

    rsx! {
        div { class: "flex w-full flex-col gap-1 py-1",
            input {
                class: "field",
                "aria-label": "New arm name",
                placeholder: "left_follower",
                autofocus: true,
                disabled: creating(),
                value: "{id}",
                oninput: move |event| id.set(event.value()),
                // Naming an arm and pressing Enter is the whole interaction;
                // reaching for the mouse to finish it would be a step too many.
                onkeydown: move |event| {
                    match event.key() {
                        Key::Enter => create(),
                        Key::Escape => open.set(false),
                        _ => {}
                    }
                },
            }
            select {
                class: "field",
                "aria-label": "New arm type",
                disabled: creating(),
                onchange: move |event| robot_type.set(event.value()),
                for (value, label) in ROBOT_TYPES {
                    option { key: "{value}", value: "{value}", selected: robot_type() == value, "{label}" }
                }
            }
            div { class: "flex gap-1",
                Action {
                    purpose: ActionPurpose::Create,
                    variant: ActionVariant::Solid,
                    disabled: creating(),
                    aria_busy: creating(),
                    onclick: move |_| create(),
                    if creating() { "Adding…" } else { "Add" }
                }
                Action {
                    purpose: ActionPurpose::Cancel,
                    disabled: creating(),
                    onclick: move |_| {
                        id.set(String::new());
                        open.set(false);
                    },
                    "Cancel"
                }
            }
        }
    }
}

/// Import a complete robot JSON document as a new instance.
#[component]
fn ImportRobotConfiguration() -> Element {
    let studio = RobotWorkspace::current();
    let importing = use_signal(|| false);
    let mut chosen = studio.selection();

    rsx! {
        FileAction {
            purpose: ActionPurpose::Import,
            variant: ActionVariant::Row,
            class: "device-list-action",
            title: "Import a unified Spiral Robotics JSON configuration",
            disabled: importing(),
            aria_busy: importing(),
            input: FileInput::new(".json,application/json", move |event: Event<FormData>| {
                let Some(file) = event.files().first().cloned() else { return };
                studio.run_utility("", async move {
                    let Some(result) = SingleFlight::run(importing, async {
                            let bytes = file
                                .read_bytes()
                                .await
                                .map_err(|error| format!("Could not read {}: {error}", file.name()))?;
                            RobotClient::current().import_robot_configuration(bytes.to_vec())
                                .await
                                .map_err(|error| error.to_string())
                        })
                        .await
                    else {
                        return Ok(());
                    };
                    match result {
                        Ok(robot) => {
                            chosen.set(Some(Selection::Robot(robot.id().to_owned())));
                            studio.report_success(format!("Imported {}.", robot.id()));
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                });
            }),
            if importing() { "Importing…" } else { "Import configuration" }
        }
    }
}

/// Everything about one arm stored in its SpiralDB configuration row.
#[component]
fn RobotSetup(robot: RobotSummary, port_catalog: PortCatalogState) -> Element {
    let studio = RobotWorkspace::current();
    let telemetry = studio.robot_telemetry(robot.id());
    let progress = telemetry.and_then(|robot| robot.calibration().cloned());
    let connected = robot.connection().connected();
    rsx! {
        div { class: "device-setup",
            // The connection used to get a row of its own reading "Motor bus
            // connected" above a section headed "Motor bus" — the same two words
            // twice, plus a dot and a state the arm's own row already carries in
            // its readiness label and its connection dot. Connect now sits
            // beside the port it opens, which is also the order the work happens
            // in.
            if let Some(problem) = &robot.problem() {
                div { class: "device-setup-alarm",
                    p { "This arm's configuration needs attention before it can connect." }
                    details { class: "technical-details",
                        summary { "Technical details" }
                        code { "{problem}" }
                    }
                }
            }
            if let Some(error) = &robot.connection().error() {
                {
                    let message = error.friendly_connection_error();
                    rsx! {
                        div { class: "device-setup-alarm",
                            p { "{message}" }
                            details { class: "technical-details",
                                summary { "Technical details" }
                                code { "{error}" }
                            }
                        }
                    }
                }
            }

            div { class: "device-setup-section",
                PortSection {
                    robot: robot.clone(),
                    state: PortSectionState::new(
                        port_catalog.scan.clone(),
                        progress.is_some(),
                        move |()| port_catalog.on_rescan.call(()),
                    ),
                }
            }
            div { class: "device-setup-section",
                CameraSection { robot: robot.clone() }
            }
            div { class: "device-setup-section",
                CalibrationSection { robot: robot.clone(), connected, progress }
            }
            div { class: "device-file-row",
                span { class: "min-w-0 flex-1 truncate text-[10px] text-ink-faint", "Stored in SpiralDB" }
                DeleteRobotAction { robot: robot.clone() }
            }
        }
    }
}

trait RobotTypePresentation {
    fn robot_role(self) -> &'static str;
}

impl RobotTypePresentation for Option<&str> {
    fn robot_role(self) -> &'static str {
        match self {
            Some(kind) if kind.contains("leader") => "Leader · hand-guided",
            Some(kind) if kind.contains("follower") => "Follower · driven",
            _ => "Robot arm",
        }
    }
}

trait RobotConnectionErrorPresentation {
    fn friendly_connection_error(&self) -> &'static str;
}

impl RobotConnectionErrorPresentation for str {
    fn friendly_connection_error(&self) -> &'static str {
        let lowered = self.to_ascii_lowercase();
        if lowered.contains("device or resource busy") || lowered.contains("already in use") {
            "This arm's serial port is already in use. Close the other connection, then try again."
        } else if lowered.contains("no such file") || lowered.contains("not found") {
            "The saved serial port is not available. Choose the connected device, then try again."
        } else if lowered.contains("permission") || lowered.contains("access denied") {
            "The studio cannot open this serial port. Check its permissions, then try again."
        } else {
            "This arm could not connect. Check its power and serial port, then try again."
        }
    }
}

/// The result of the shared serial-port scan: still running, failed, or done.
type PortScan = Option<Result<Vec<SerialPortSummary>, String>>;

#[derive(Clone, PartialEq)]
struct PortCatalogState {
    scan: PortScan,
    on_rescan: EventHandler<()>,
}

impl PortCatalogState {
    fn new(scan: PortScan, on_rescan: impl FnMut(()) + 'static) -> Self {
        Self {
            scan,
            on_rescan: EventHandler::new(on_rescan),
        }
    }
}

#[component]
fn ConnectionAction(robot: RobotSummary, sweeping: bool) -> Element {
    let studio = RobotWorkspace::current();
    let pending = use_signal(|| false);
    let id = robot.id().to_owned();
    let connected = robot.connection().connected();
    // Disconnecting mid-sweep throws the sweep away, so make the operator
    // finish or cancel deliberately rather than lose it to one click.
    let blocked = robot.port().is_none() || robot.problem().is_some() || (connected && sweeping);
    let reason = if robot.port().is_none() {
        "Assign a serial port first"
    } else if robot.problem().is_some() {
        "Fix this arm's configuration first"
    } else if connected && sweeping {
        "Finish or cancel calibration first"
    } else {
        ""
    };

    rsx! {
        Action {
            purpose: if connected { ActionPurpose::Disconnect } else { ActionPurpose::Connect },
            variant: if connected { ActionVariant::Outline } else { ActionVariant::Solid },
            disabled: blocked || pending(),
            aria_busy: pending(),
            title: "{reason}",
            onclick: move |_| {
                if pending.peek().to_owned() {
                    return;
                }
                let id = id.clone();
                studio.run("", async move {
                    SingleFlight::run(pending, async move {
                        let result = if connected {
                            RobotClient::current().disconnect_robot(id).await
                        } else {
                            RobotClient::current().connect_robot(id).await
                        };
                        result.map_err(|error| error.to_string())
                    })
                        .await
                        .unwrap_or(Ok(()))
                });
            },
            {if pending() { if connected { "Disconnecting…" } else { "Connecting…" } } else if connected { "Disconnect" } else { "Connect" }}
        }
    }
}

#[derive(Clone, PartialEq)]
struct PortSectionState {
    scan: PortScan,
    sweeping: bool,
    on_rescan: EventHandler<()>,
}

impl PortSectionState {
    fn new(scan: PortScan, sweeping: bool, on_rescan: impl FnMut(()) + 'static) -> Self {
        Self {
            scan,
            sweeping,
            on_rescan: EventHandler::new(on_rescan),
        }
    }
}

#[component]
fn PortSection(robot: RobotSummary, state: PortSectionState) -> Element {
    let PortSectionState {
        scan,
        sweeping,
        on_rescan,
    } = state;
    let studio = RobotWorkspace::current();
    let mut identify_step = use_signal(|| IdentifyStep::Idle);
    let starting_identify = use_signal(|| false);
    let current = robot
        .port()
        .map(|port| port.path().to_owned())
        .unwrap_or_default();
    let baud = robot
        .port()
        .as_ref()
        .map_or(1_000_000, |port| port.baud_rate());
    let id = robot.id().to_owned();
    // The port belongs to the open bus once connected; changing it would make
    // the durable configuration disagree with the hardware, so the server refuses it too.
    let locked = robot.connection().connected();
    let scanning = scan.is_none();
    let found = match &scan {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    // A port recorded earlier but missing from the scan still has to appear,
    // otherwise the field would silently look unset.
    let missing = !current.is_empty() && !found.iter().any(|port| port.path() == current);

    rsx! {
        section { class: "device-section-content",
            div { class: "device-section-head",
                h3 { class: "label mb-0", "Motor bus" }
                Action {
                    purpose: ActionPurpose::Refresh,
                    variant: ActionVariant::Link,
                    class: "ml-auto",
                    disabled: scanning,
                    aria_busy: scanning,
                    onclick: move |_| on_rescan.call(()),
                    if scanning { "Scanning…" } else { "Rescan" }
                }
                Action {
                    purpose: ActionPurpose::Start,
                    variant: ActionVariant::Link,
                    disabled: locked || scanning || starting_identify(),
                    aria_busy: starting_identify(),
                    // Names the action the button performs. Borrowing the port
                    // field's wording told an operator hovering "Identify" why
                    // some other control was unavailable.
                    title: if locked { "Disconnect this arm before identifying its port" } else { "Find the port by unplugging the cable" },
                    onclick: move |_| {
                        identify_step.set(IdentifyStep::Idle);
                        if !studio.controls_available() {
                            studio.report("Controls are paused because telemetry is unavailable or delayed.");
                            return;
                        }
                        spawn(async move {
                            let Some(result) = SingleFlight::run(
                                starting_identify,
                                async move {
                                    RobotClient::current().list_serial_ports()
                                        .await
                                        .map_err(|error| error.to_string())
                                },
                            )
                            .await
                            else {
                                return;
                            };
                            match result {
                                Ok(list) => identify_step.set(IdentifyStep::AwaitingUnplug(
                                    list.into_iter().map(|port| port.path().to_owned()).collect(),
                                )),
                                Err(error) => studio.report(error),
                            }
                        });
                    },
                    if starting_identify() { "Starting…" } else { "Identify" }
                }
            }
            // The port and the button that opens it share a line: choosing one
            // and connecting to it is a single piece of work, and the button's
            // own label is the only place the connection state needs saying
            // inside the setup.
            div { class: "device-port-row",
                select {
                    class: "field",
                    "aria-label": "Serial port",
                    disabled: locked,
                    onchange: {
                        let id = id.clone();
                        move |event: Event<FormData>| {
                            let value = event.value();
                            if !value.is_empty() {
                                studio.assign_port(PortAssignment::new(id.clone(), value, baud));
                            }
                        }
                    },
                    option { value: "", selected: current.is_empty(), "Choose a serial port…" }
                    for port in found {
                        PortOption {
                            key: "{port.path()}",
                            selected: port.path() == current,
                            port,
                            owner: robot.id().to_owned(),
                        }
                    }
                    if missing {
                        option { value: "{current}", selected: true, "{current} (not detected)" }
                    }
                    if matches!(scan, Some(Err(_))) {
                        option { disabled: true, "Could not scan for ports" }
                    }
                    if scan.is_none() {
                        option { disabled: true, "Scanning…" }
                    }
                }
                ConnectionAction { robot: robot.clone(), sweeping }
            }
            if let Some(Err(error)) = &scan {
                p { class: "mt-0.5 text-[11px] text-danger", "{error}" }
            }

            IdentifyPort {
                robot,
                step: identify_step,
                on_found: {
                    let id = id.clone();
                    move |path: String| {
                        studio.assign_port(PortAssignment::new(id.clone(), path, baud))
                    }
                },
            }
        }
    }
}

/// One serial port in the picker.
///
/// A port another robot already holds open is shown but not selectable:
/// choosing it would only produce a refusal at connect time.
#[component]
fn PortOption(port: SerialPortSummary, owner: String, selected: bool) -> Element {
    let holder = port.used_by().filter(|holder| *holder != owner);
    let mut label = port.path().to_owned();
    if let Some(description) = port.description() {
        label.push_str(" — ");
        label.push_str(description);
    }
    if let Some(holder) = holder {
        label.push_str(&format!(" (in use by {holder})"));
    }

    rsx! {
        option { value: "{port.path()}", disabled: holder.is_some(), selected, "{label}" }
    }
}

/// Record a serial port on a robot, keeping its existing baud rate.
impl RobotWorkspace {
    fn assign_port(self, assignment: PortAssignment) {
        let request = RobotUpdate::with_port(
            assignment.robot_id,
            PortSettings::new(assignment.path.clone(), assignment.baud_rate),
        );
        self.run(format!("Assigned {}.", assignment.path), async move {
            RobotClient::current()
                .update_robot(request)
                .await
                .map_err(|error| error.to_string())
                .map(|_| ())
        });
    }
}

struct PortAssignment {
    robot_id: String,
    path: String,
    baud_rate: u32,
}

impl PortAssignment {
    fn new(robot_id: String, path: String, baud_rate: u32) -> Self {
        Self {
            robot_id,
            path,
            baud_rate,
        }
    }
}

/// Where the operator is in the unplug-and-compare workflow.
#[derive(Clone, Debug, PartialEq)]
enum IdentifyStep {
    Idle,
    AwaitingUnplug(Vec<String>),
    Finished(String),
}

#[component]
fn IdentifyPort(
    robot: RobotSummary,
    step: Signal<IdentifyStep>,
    on_found: EventHandler<String>,
) -> Element {
    let studio = RobotWorkspace::current();
    let mut step = step;
    let checking = use_signal(|| false);

    match step().clone() {
        IdentifyStep::Idle => rsx! {},
        IdentifyStep::AwaitingUnplug(before) => rsx! {
            div { class: "mt-1 rounded border border-line bg-sunken p-1.5 text-[11px]",
                p { class: "mb-1",
                    "Unplug the USB cable from "
                    strong { class: "font-medium", "{robot.id()}" }
                    ", then continue. Leave every other device connected."
                }
                div { class: "flex gap-1",
                    Action {
                        purpose: ActionPurpose::Apply,
                        variant: ActionVariant::Solid,
                        disabled: checking(),
                        aria_busy: checking(),
                        onclick: move |_| {
                            if !studio.controls_available() {
                                studio.report("Controls are paused because telemetry is unavailable or delayed.");
                                return;
                            }
                            let before = before.clone();
                            spawn(async move {
                                let Some(result) = SingleFlight::run(
                                    checking,
                                    async move {
                                        RobotClient::current().identify_serial_port(before)
                                            .await
                                            .map_err(|error| error.to_string())
                                    },
                                )
                                .await
                                else {
                                    return;
                                };
                                match result {
                                    Ok(identification) => match identification.port() {
                                        Some(port) => {
                                            on_found.call(port.to_owned());
                                            step.set(IdentifyStep::Finished(identification.message().to_owned()));
                                        }
                                        None => studio.report(identification.message()),
                                    },
                                    Err(error) => studio.report(error),
                                }
                            });
                        },
                        if checking() { "Checking…" } else { "I unplugged it" }
                    }
                    Action {
                        purpose: ActionPurpose::Cancel,
                        disabled: checking(),
                        onclick: move |_| step.set(IdentifyStep::Idle),
                        "Cancel"
                    }
                }
            }
        },
        IdentifyStep::Finished(message) => rsx! {
            div { class: "mt-1 flex items-start gap-1.5 rounded border border-line bg-sunken p-1.5 text-[11px]",
                span { class: "flex-1", "{message}" }
                Action {
                    purpose: ActionPurpose::Close,
                    variant: ActionVariant::Link,
                    onclick: move |_| step.set(IdentifyStep::Idle),
                    "Done"
                }
            }
        },
    }
}

#[component]
fn CameraSection(robot: RobotSummary) -> Element {
    let studio = RobotWorkspace::current();
    let available = studio.snapshot().read().cameras().to_owned();
    let mut role = use_signal(|| "wrist".to_owned());
    let mut device = use_signal(String::new);
    let saving_cameras = use_signal(|| false);
    let id = robot.id().to_owned();
    let chosen = available
        .iter()
        .find(|camera| camera.id() == device())
        .cloned();

    let scanning = use_signal(|| false);
    let taken = robot
        .cameras()
        .iter()
        .any(|camera| camera.name() == role.read().trim());
    let replaces = if taken {
        format!(
            "Point the {} role at this camera instead",
            role.read().trim()
        )
    } else {
        String::new()
    };

    rsx! {
        section { class: "device-section-content",
            div { class: "device-section-head",
                h3 { class: "label mb-0 flex-1", "Cameras" }
                Action {
                    purpose: ActionPurpose::Refresh,
                    variant: ActionVariant::Link,
                    disabled: scanning(),
                    aria_busy: scanning(),
                    title: "Look for cameras that have been plugged in or moved",
                    onclick: move |_| {
                        if !studio.controls_available() {
                            studio.report("Controls are paused because telemetry is unavailable or delayed.");
                            return;
                        }
                        spawn(async move {
                            if let Some(Err(error)) = SingleFlight::run(
                                scanning,
                                async move {
                                    RobotClient::current().refresh_cameras()
                                        .await
                                        .map_err(|error| error.to_string())
                                        .map(|_| ())
                                },
                            )
                            .await
                            {
                                studio.report(error)
                            }
                        });
                    },
                    {if scanning() { "Scanning…" } else { "Rescan" }}
                }
            }
            // Unassigning a camera is rare and reversible in one click, so it
            // does not hold a permanent word on every row the way it used to —
            // the same treatment an episode card already gives its delete.
            ul { class: "mb-1 flex flex-col gap-0.5",
                for binding in robot.cameras().to_owned() {
                    li { key: "{binding.name()}", class: "device-camera-row ui-hover-action-host",
                        span { class: "shrink-0", "{binding.name()}" }
                        span { class: "min-w-0 flex-1 truncate font-mono text-[10px] text-ink-faint",
                            "{binding.id()}"
                        }
                        Action {
                            purpose: ActionPurpose::Remove,
                            variant: ActionVariant::Icon,
                            class: "device-camera-remove ui-hover-action",
                            disabled: saving_cameras(),
                            aria_busy: saving_cameras(),
                            aria_label: "Unassign the {binding.name()} camera",
                            title: "Unassign the {binding.name()} camera",
                            onclick: {
                                let id = id.clone();
                                let remaining = robot.cameras().iter()
                                    .filter(|camera| camera.name() != binding.name())
                                    .cloned()
                                    .collect::<Vec<_>>();
                                move |_| {
                                    studio.save_cameras(CameraAssignments::new(
                                        id.clone(),
                                        remaining.clone(),
                                        saving_cameras,
                                    ))
                                }
                            },
                            UnassignIcon {}
                        }
                    }
                }
            }
            if available.is_empty() {
                p { class: "text-[11px] text-ink-faint", "No cameras detected. Plug one in and rescan." }
            } else {
                select {
                    class: "field",
                    "aria-label": "Camera device",
                    disabled: saving_cameras(),
                    onchange: move |event| device.set(event.value()),
                    option { value: "", selected: device().is_empty(), "Add a camera…" }
                    for camera in available.clone() {
                        option {
                            key: "{camera.id()}",
                            value: "{camera.id()}",
                            selected: device() == camera.id(),
                            "{camera.name()} — {camera.width()}×{camera.height()} @ {camera.fps()} fps"
                        }
                    }
                }
            }

            // Picking a camera by device name is guesswork; seeing through it
            // before assigning it is not. The preview appears as soon as one is
            // chosen, which is also what starts the stream on the server. The
            // role is asked for here rather than up front, because it is only
            // answerable once you can see what the camera is looking at.
            if let Some(camera) = chosen {
                div { class: "mt-1 overflow-hidden rounded border border-line",
                    img {
                        class: "aspect-video w-full bg-camera object-contain",
                        src: {
                            use crate::client::http::CameraApiPath;
                            camera.id().camera_stream_url()
                        },
                        alt: "Live preview from {camera.name()}",
                    }
                    div { class: "flex items-center gap-1 p-1",
                        input {
                            class: "field",
                            "aria-label": "Camera role",
                            placeholder: "role, such as wrist",
                            disabled: saving_cameras(),
                            value: "{role}",
                            oninput: move |event| role.set(event.value()),
                        }
                        Action {
                            purpose: ActionPurpose::Assign,
                            variant: ActionVariant::Solid,
                            class: "shrink-0",
                            disabled: saving_cameras(),
                            aria_busy: saving_cameras(),
                            // A role names one viewpoint on the arm, so reusing
                            // one takes the camera off whatever held it. That is
                            // often what is wanted — but the button has to say
                            // so rather than quietly doing it.
                            title: "{replaces}",
                            onclick: {
                                let id = id.clone();
                                let existing = robot.cameras().to_owned();
                                move |_| {
                                    let (name, device_id) = (role.read().trim().to_owned(), device());
                                    if name.is_empty() {
                                        studio.report("Give the camera a role name.");
                                        return;
                                    }
                                    let mut bindings = existing.iter()
                                        .filter(|camera| camera.name() != name)
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    bindings.push(CameraBinding::new(name, device_id, None));
                                    studio.save_cameras(CameraAssignments::new(
                                        id.clone(),
                                        bindings,
                                        saving_cameras,
                                    ));
                                }
                            },
                            {if saving_cameras() { "Saving…" } else if taken { "Reassign" } else { "Assign" }}
                        }
                    }
                }
            }
        }
    }
}

/// Replace a robot's camera assignments.
impl RobotWorkspace {
    fn save_cameras(self, assignments: CameraAssignments) {
        if assignments.saving.peek().to_owned() {
            return;
        }
        let request = RobotUpdate::with_cameras(assignments.robot_id, assignments.bindings);
        self.run("", async move {
            SingleFlight::run(assignments.saving, async move {
                RobotClient::current()
                    .update_robot(request)
                    .await
                    .map_err(|error| error.to_string())
                    .map(|_| ())
            })
            .await
            .unwrap_or(Ok(()))
        });
    }
}

struct CameraAssignments {
    robot_id: String,
    bindings: Vec<CameraBinding>,
    saving: Signal<bool>,
}

impl CameraAssignments {
    fn new(robot_id: String, bindings: Vec<CameraBinding>, saving: Signal<bool>) -> Self {
        Self {
            robot_id,
            bindings,
            saving,
        }
    }
}

mod client;
pub(crate) use client::RobotClient;
