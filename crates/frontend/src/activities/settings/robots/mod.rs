//! Creating robot instances and getting them ready to move.
//!
//! One arm is one robot instance in Studio. Everything on this panel—the
//! port, cameras, motors, and calibration—is one durable configuration row.
//!
//! The panel lives in Settings as the durable owner of ports, camera
//! assignments, and calibration. Live consumes that configuration without
//! mixing setup work into the operating surface.

use dioxus::prelude::*;
use spiral_robotics_common::robots::config::{RobotConfig, RobotConfigSave};
use spiral_robotics_common::robots::{PortSettings, RobotSummary, SerialPortSummary};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ChevronIcon, Disclosure, FileButton, FileInput, Select,
    TextInput, UnassignIcon,
};

use crate::panels::scene::ScenePalette;
use crate::robots::{HardwareCommands, HardwareViewState, RobotSelection, Selection};
use crate::shell::project::ProjectSelection;
use crate::shell::state::StudioProjectState;

mod calibration;
mod delete;
mod discovery;
use calibration::CalibrationSection;
use delete::DeleteRobotAction;
use discovery::IdentifiedSerialPort;

/// Robot types offered when creating an instance.
const ROBOT_TYPES: [(&str, &str); 2] = [
    ("so101_follower", "Follower (driven arm)"),
    ("so101_leader", "Leader (hand-guided arm)"),
];

#[component]
pub(super) fn RobotPanel(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
) -> Element {
    let robots = telemetry.snapshot().read().robots().to_owned();
    let focused = selection.focused(&robots);
    let project_selection = ProjectSelection::current();
    let mut ports = use_resource(move || {
        let available = project_selection.live();
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
                    RobotEntry { project, telemetry, selection,
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
                NewRobotForm { selection, project }
                ImportRobotConfiguration { selection, project }
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
fn RobotEntry(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    robot: RobotSummary,
    state: RobotEntryState,
) -> Element {
    let RobotEntryState {
        focused,
        accent,
        port_catalog,
    } = state;
    let mut chosen = selection.selected();
    // Open on whatever still needs attention. An arm that is wired up and
    // calibrated has nothing to do here, so it starts folded; anything else
    // opens on the work. Only the initial state is derived — an arm becoming
    // ready while the operator is reading it must not fold itself away.
    let ready = robot.config().port().is_some() && robot.config().calibrated();
    let mut open = use_signal(|| !ready || !robot.connection().connected());
    let id = robot.id().to_owned();
    let role = robot.config().robot_type().robot_role();
    let state = if robot.config().port().is_none() {
        "Port needed"
    } else if !robot.config().calibrated() {
        "Calibration needed"
    } else if robot.connection().connected() {
        "Connected"
    } else if robot.connection().error().is_some() {
        // The last attempt failed; the reason is in the setup below.
        "Connection failed"
    } else {
        "Ready to connect"
    };

    rsx! {
        li { class: "device-entry",
            div {
                class: "device-entry-row",
                Button {
                    purpose: ActionPurpose::Toggle,
                    variant: ButtonVariant::Icon,
                    class: "device-chevron",
                    aria_expanded: open(),
                    aria_label: if open() { "Hide setup for {robot.id()}" } else { "Show setup for {robot.id()}" },
                    title: if open() { "Hide this arm's setup" } else { "Show this arm's setup" },
                    onclick: move |_| open.toggle(),
                    ChevronIcon { open: open() }
                }
                Button {
                    purpose: ActionPurpose::Select,
                    variant: ButtonVariant::Row,
                    class: "device-main",
                    aria_pressed: focused,
                    title: "Select {robot.id()}",
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
                    span { class: if ready && robot.connection().error().is_none() { "device-state" } else { "device-state device-state-needed" }, "{state}" }
                    span {
                        class: "device-connection-dot",
                        style: if robot.connection().connected() { "background: var(--tone-good)" } else { "background: var(--tone-line)" },
                        title: if robot.connection().connected() { "Connected" } else { "Not connected" },
                    }
                }
            }
            if open() {
                RobotSetup { project, telemetry, robot: robot.clone(), port_catalog }
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
fn NewRobotForm(project: StudioProjectState, selection: RobotSelection) -> Element {
    let mut chosen = selection.selected();
    let mut open = use_signal(|| false);
    let mut id = use_signal(String::new);
    let mut robot_type = use_signal(|| ROBOT_TYPES[0].0.to_owned());
    let mut create_robot = use_action(move |name: String, robot_type: String| async move {
        project
            .execute_utility(format!("Created {name}."), async move {
                let config = RobotConfig::so101(&name, &robot_type)?;
                RobotClient::current()
                    .save_robot(RobotConfigSave::new(config, None))
                    .await
                    .map_err(|error| error.to_string())?;
                // Keep the form intact until the server accepts the new robot.
                id.set(String::new());
                open.set(false);
                chosen.set(Some(Selection::Robot(name)));
                Ok(())
            })
            .await
    });
    let mut create = move || {
        let name = id.read().trim().to_owned();
        if name.is_empty() {
            project.report("Give the arm a name first.");
            return;
        }
        create_robot.call(name, robot_type());
    };

    if !open() {
        return rsx! {
            Button {
                purpose: ActionPurpose::Create,
                variant: ButtonVariant::Row,
                class: "device-list-action",
                onclick: move |_| open.set(true),
                "+ Add an arm"
            }
        };
    }

    rsx! {
        div { class: "flex w-full flex-col gap-1 py-1",
            TextInput {
                aria_label: "New arm name",
                placeholder: "left_follower",
                autofocus: true,
                disabled: create_robot.pending(),
                value: "{id}",
                oninput: move |value| id.set(value),
                // Naming an arm and pressing Enter is the whole interaction;
                // reaching for the mouse to finish it would be a step too many.
                onkeydown: move |event: KeyboardEvent| {
                    match event.key() {
                        Key::Enter => create(),
                        Key::Escape => open.set(false),
                        _ => {}
                    }
                },
            }
            Select {
                aria_label: "New arm type",
                value: robot_type(),
                disabled: create_robot.pending(),
                onchange: move |value| robot_type.set(value),
                for (value, label) in ROBOT_TYPES {
                    option { key: "{value}", value: "{value}", selected: robot_type() == value, "{label}" }
                }
            }
            div { class: "flex gap-1",
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Solid,
                    disabled: create_robot.pending(),
                    aria_busy: create_robot.pending(),
                    onclick: move |_| create(),
                    if create_robot.pending() { "Adding…" } else { "Add" }
                }
                Button {
                    purpose: ActionPurpose::Cancel,
                    disabled: create_robot.pending(),
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

/// Import a complete robot JSON configuration as a new instance.
#[component]
fn ImportRobotConfiguration(project: StudioProjectState, selection: RobotSelection) -> Element {
    let mut chosen = selection.selected();
    let mut import_robot = use_action(move |file: dioxus::html::FileData| async move {
        project
            .execute_utility("", async move {
                let bytes = file
                    .read_bytes()
                    .await
                    .map_err(|error| format!("Could not read {}: {error}", file.name()))?;
                let config = serde_json::from_slice::<RobotConfig>(&bytes).map_err(|error| {
                    format!("{} is not a robot configuration: {error}", file.name())
                })?;
                let robot = RobotClient::current()
                    .save_robot(RobotConfigSave::new(config, None))
                    .await
                    .map_err(|error| error.to_string())?;
                chosen.set(Some(Selection::Robot(robot.id().to_owned())));
                project.report_success(format!("Imported {}.", robot.id()));
                if robot.config().port().is_some() {
                    RobotClient::current()
                        .connect_robot(robot.id().to_owned())
                        .await
                        .map_err(|error| {
                            format!(
                                "Configuration imported, but the robot could not connect: {error}"
                            )
                        })?;
                }
                Ok(())
            })
            .await
    });

    rsx! {
        FileButton {
            purpose: ActionPurpose::Import,
            variant: ButtonVariant::Row,
            class: "device-list-action",
            title: "Import a unified Spiral Robotics JSON configuration",
            disabled: import_robot.pending(),
            aria_busy: import_robot.pending(),
            input: FileInput::new(".json,application/json", move |event: Event<FormData>| {
                let Some(file) = event.files().first().cloned() else { return };
                import_robot.call(file);
            }),
            if import_robot.pending() { "Importing…" } else { "Import configuration" }
        }
    }
}

/// Everything about one arm stored in its local Studio configuration row.
#[component]
fn RobotSetup(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
    port_catalog: PortCatalogState,
) -> Element {
    let progress = telemetry
        .robot_telemetry(robot.id())
        .and_then(|robot| robot.calibration().cloned());
    rsx! {
        div { class: "device-setup",
            // The connection used to get a row of its own reading "Motor bus
            // connected" above a section headed "Motor bus" — the same two words
            // twice, plus a dot and a state the arm's own row already carries in
            // its readiness label and its connection dot. Connect now sits
            // beside the port it opens, which is also the order the work happens
            // in.
            if let Some(error) = &robot.connection().error() {
                {
                    let message = error.friendly_connection_error();
                    rsx! {
                        div { class: "device-setup-alarm",
                            p { "{message}" }
                            Disclosure { summary: rsx! { "Technical details" },
                                code { "{error}" }
                            }
                        }
                    }
                }
            }

            div { class: "device-setup-section",
                PortSection { project, telemetry,
                    robot: robot.clone(),
                    state: PortSectionState::new(
                        port_catalog.scan.clone(),
                        progress.is_some(),
                        move |()| port_catalog.on_rescan.call(()),
                    ),
                }
            }
            div { class: "device-setup-section",
                CameraSection { project, telemetry, robot: robot.clone() }
            }
            div { class: "device-setup-section",
                CalibrationSection { project, telemetry, robot: robot.clone() }
            }
            div { class: "device-file-row",
                span { class: "min-w-0 flex-1 truncate text-[10px] text-ink-faint", "Saved on this backend" }
                DeleteRobotAction { project, telemetry, robot: robot.clone() }
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
fn ConnectionAction(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
    sweeping: bool,
) -> Element {
    let connected = robot.connection().connected();
    let id = robot.id().to_owned();
    let mut change_connection = use_action(move |id: String| async move {
        HardwareCommands::new(project, telemetry)
            .execute("", async move {
                let result = if connected {
                    RobotClient::current().disconnect_robot(id).await
                } else {
                    RobotClient::current().connect_robot(id).await
                };
                result.map_err(|error| error.to_string())
            })
            .await
    });
    // Disconnecting mid-sweep throws the sweep away, so make the operator
    // finish or cancel deliberately rather than lose it to one click.
    let blocked = robot.config().port().is_none() || (connected && sweeping);
    let reason = if robot.config().port().is_none() {
        "Assign a serial port first"
    } else if connected && sweeping {
        "Finish or cancel calibration first"
    } else {
        ""
    };

    rsx! {
        Button {
            purpose: if connected { ActionPurpose::Disconnect } else { ActionPurpose::Connect },
            variant: if connected { ButtonVariant::Outline } else { ButtonVariant::Solid },
            disabled: blocked || change_connection.pending(),
            aria_busy: change_connection.pending(),
            title: "{reason}",
            onclick: move |_| { change_connection.call(id.clone()); },
            {if change_connection.pending() { if connected { "Disconnecting…" } else { "Connecting…" } } else if connected { "Disconnect" } else { "Connect" }}
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
fn PortSection(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
    state: PortSectionState,
) -> Element {
    let PortSectionState {
        scan,
        sweeping,
        on_rescan,
    } = state;
    let mut identify_step = use_signal(|| IdentifyStep::Idle);
    let mut start_identify = use_action(move || async move {
        identify_step.set(IdentifyStep::Idle);
        HardwareCommands::new(project, telemetry)
            .execute("", async move {
                let list = RobotClient::current()
                    .list_serial_ports()
                    .await
                    .map_err(|error| error.to_string())?;
                identify_step.set(IdentifyStep::AwaitingUnplug(
                    list.into_iter()
                        .map(|port| port.path().to_owned())
                        .collect(),
                ));
                Ok(())
            })
            .await
    });
    let current = robot
        .config()
        .port()
        .map(|port| port.path().to_owned())
        .unwrap_or_default();
    let baud = robot
        .config()
        .port()
        .map_or(1_000_000, PortSettings::baud_rate);
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
                Button {
                    purpose: ActionPurpose::Refresh,
                    variant: ButtonVariant::Link,
                    class: "ml-auto",
                    disabled: scanning,
                    aria_busy: scanning,
                    onclick: move |_| on_rescan.call(()),
                    if scanning { "Scanning…" } else { "Rescan" }
                }
                Button {
                    purpose: ActionPurpose::Start,
                    variant: ButtonVariant::Link,
                    disabled: locked || scanning || start_identify.pending(),
                    aria_busy: start_identify.pending(),
                    // Names the action the button performs. Borrowing the port
                    // field's wording told an operator hovering "Identify" why
                    // some other control was unavailable.
                    title: if locked { "Disconnect this arm before identifying its port" } else { "Find the port by unplugging the cable" },
                    onclick: move |_| { start_identify.call(); },
                    if start_identify.pending() { "Starting…" } else { "Identify" }
                }
            }
            // The port and the button that opens it share a line: choosing one
            // and connecting to it is a single piece of work, and the button's
            // own label is the only place the connection state needs saying
            // inside the setup.
            div { class: "device-port-row",
                Select {
                    aria_label: "Serial port",
                    value: current.to_owned(),
                    disabled: locked,
                    onchange: {
                        let robot = robot.clone();
                        move |value: String| {
                            if !value.is_empty() {
                                HardwareCommands::new(project, telemetry).assign_port(PortAssignment::new(robot.clone(), value, baud));
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
                ConnectionAction { project, telemetry, robot: robot.clone(), sweeping }
            }
            if let Some(Err(error)) = &scan {
                p { class: "mt-0.5 text-[11px] text-danger", "{error}" }
            }

            IdentifyPort { project, telemetry,
                robot: robot.clone(),
                step: identify_step,
                on_found: {
                    let robot = robot.clone();
                    move |path: String| {
                        HardwareCommands::new(project, telemetry).assign_port(PortAssignment::new(robot.clone(), path, baud))
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
impl HardwareCommands {
    fn assign_port(self, assignment: PortAssignment) {
        let port = PortSettings::new(assignment.path.trim(), assignment.baud_rate);
        let request = assignment
            .robot
            .config()
            .clone()
            .with_port(port)
            .map(|config| RobotConfigSave::new(config, Some(assignment.robot.revision())));
        self.run(format!("Assigned {}.", assignment.path), async move {
            let client = RobotClient::current();
            let saved = client
                .save_robot(request?)
                .await
                .map_err(|error| error.to_string())?;
            client
                .connect_robot(saved.id().to_owned())
                .await
                .map_err(|error| format!("Port saved, but the robot could not connect: {error}"))
        });
    }
}

struct PortAssignment {
    robot: RobotSummary,
    path: String,
    baud_rate: u32,
}

impl PortAssignment {
    fn new(robot: RobotSummary, path: String, baud_rate: u32) -> Self {
        Self {
            robot,
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
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
    step: Signal<IdentifyStep>,
    on_found: EventHandler<String>,
) -> Element {
    let mut step = step;
    let mut identify_port = use_action(move |before: Vec<String>| async move {
        HardwareCommands::new(project, telemetry)
            .execute("", async move {
                let after = RobotClient::current()
                    .list_serial_ports()
                    .await
                    .map_err(|error| error.to_string())?;
                let identification = IdentifiedSerialPort::between(
                    before,
                    after
                        .into_iter()
                        .map(|port| port.path().to_owned())
                        .collect(),
                );
                match identification.port() {
                    Some(port) => {
                        on_found.call(port.to_owned());
                        step.set(IdentifyStep::Finished(identification.message().to_owned()));
                        Ok(())
                    }
                    None => Err(identification.message().to_owned()),
                }
            })
            .await
    });

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
                    Button {
                        purpose: ActionPurpose::Apply,
                        variant: ButtonVariant::Solid,
                        disabled: identify_port.pending(),
                        aria_busy: identify_port.pending(),
                        onclick: move |_| { identify_port.call(before.clone()); },
                        if identify_port.pending() { "Checking…" } else { "I unplugged it" }
                    }
                    Button {
                        purpose: ActionPurpose::Cancel,
                        disabled: identify_port.pending(),
                        onclick: move |_| step.set(IdentifyStep::Idle),
                        "Cancel"
                    }
                }
            }
        },
        IdentifyStep::Finished(message) => rsx! {
            div { class: "mt-1 flex items-start gap-1.5 rounded border border-line bg-sunken p-1.5 text-[11px]",
                span { class: "flex-1", "{message}" }
                Button {
                    purpose: ActionPurpose::Close,
                    variant: ButtonVariant::Link,
                    onclick: move |_| step.set(IdentifyStep::Idle),
                    "Done"
                }
            }
        },
    }
}

#[component]
fn CameraSection(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot: RobotSummary,
) -> Element {
    let available = telemetry.snapshot().read().cameras().to_owned();
    let mut role = use_signal(|| "wrist".to_owned());
    let mut device = use_signal(String::new);
    let mut save_cameras = use_action(move |request: RobotConfigSave| async move {
        HardwareCommands::new(project, telemetry)
            .execute("", async move {
                RobotClient::current()
                    .save_robot(request)
                    .await
                    .map_err(|error| error.to_string())
                    .map(|_| ())
            })
            .await
    });
    let chosen = available
        .iter()
        .find(|camera| camera.id() == device())
        .cloned();

    let mut scan_cameras = use_action(move || async move {
        HardwareCommands::new(project, telemetry)
            .execute("", async move {
                RobotClient::current()
                    .refresh_cameras()
                    .await
                    .map_err(|error| error.to_string())
                    .map(|_| ())
            })
            .await
    });
    let taken = robot.config().cameras().contains_key(role.read().trim());
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
                Button {
                    purpose: ActionPurpose::Refresh,
                    variant: ButtonVariant::Link,
                    disabled: scan_cameras.pending(),
                    aria_busy: scan_cameras.pending(),
                    title: "Look for cameras that have been plugged in or moved",
                    onclick: move |_| { scan_cameras.call(); },
                    {if scan_cameras.pending() { "Scanning…" } else { "Rescan" }}
                }
            }
            // Unassigning a camera is rare and reversible in one click, so it
            // does not hold a permanent word on every row the way it used to —
            // the same treatment an episode card already gives its delete.
            ul { class: "mb-1 flex flex-col gap-0.5",
                for (camera_role, camera) in robot.config().cameras().clone() {
                    li { key: "{camera_role}", class: "device-camera-row ui-hover-action-host",
                        span { class: "shrink-0", "{camera_role}" }
                        span { class: "min-w-0 flex-1 truncate font-mono text-[10px] text-ink-faint",
                            "{camera.id()}"
                        }
                        Button {
                            purpose: ActionPurpose::Remove,
                            variant: ButtonVariant::Icon,
                            class: "device-camera-remove ui-hover-action",
                            disabled: save_cameras.pending(),
                            aria_busy: save_cameras.pending(),
                            aria_label: "Unassign the {camera_role} camera",
                            title: "Unassign the {camera_role} camera",
                            onclick: {
                                let request = RobotConfigSave::new(
                                    robot.config().clone().without_camera(&camera_role),
                                    Some(robot.revision()),
                                );
                                move |_| save_cameras.call(request.clone())
                            },
                            UnassignIcon {}
                        }
                    }
                }
            }
            if available.is_empty() {
                p { class: "text-[11px] text-ink-faint", "No cameras detected. Plug one in and rescan." }
            } else {
                Select {
                    aria_label: "Camera device",
                    value: device(),
                    disabled: save_cameras.pending(),
                    onchange: move |value| device.set(value),
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
                        TextInput {
                            aria_label: "Camera role",
                            placeholder: "role, such as wrist",
                            disabled: save_cameras.pending(),
                            value: "{role}",
                            oninput: move |value| role.set(value),
                        }
                        Button {
                            purpose: ActionPurpose::Assign,
                            variant: ButtonVariant::Solid,
                            class: "shrink-0",
                            disabled: save_cameras.pending(),
                            aria_busy: save_cameras.pending(),
                            // A role names one viewpoint on the arm, so reusing
                            // one takes the camera off whatever held it. That is
                            // often what is wanted — but the button has to say
                            // so rather than quietly doing it.
                            title: "{replaces}",
                            onclick: {
                                let robot = robot.clone();
                                move |_| {
                                    let name = role.read().trim().to_owned();
                                    if name.is_empty() {
                                        project.report("Give the camera a role name.");
                                        return;
                                    }
                                    match robot.config().clone().with_camera(&name, &device()) {
                                        Ok(config) => {
                                            save_cameras.call(RobotConfigSave::new(
                                                config,
                                                Some(robot.revision()),
                                            ));
                                        }
                                        Err(error) => project.report(error),
                                    }
                                }
                            },
                            {if save_cameras.pending() { "Saving…" } else if taken { "Reassign" } else { "Assign" }}
                        }
                    }
                }
            }
        }
    }
}

use crate::robots::RobotClient;
