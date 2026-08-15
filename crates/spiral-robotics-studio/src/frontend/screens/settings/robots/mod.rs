//! Creating robot instances and getting them ready to move.
//!
//! One arm is one robot instance in SpiralDB. Everything on this panel—the
//! port, cameras, motors, and calibration—is one durable configuration row.
//!
//! The panel lives in Settings as the durable owner of ports, camera
//! assignments, and calibration. Live consumes that configuration without
//! mixing setup work into the operating surface.

use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::{
    CameraBinding, CreateRobotRequest, IdentifyPortRequest, IdentifyPortResponse, PortSettings,
    RobotSummary, SerialPortSummary, UpdateRobotRequest,
};
use spiral_robotics_ui::{
    Action, ActionLink, ActionPurpose, ActionVariant, ChevronIcon, FileAction, UnassignIcon,
};

use crate::frontend::api::http as api;
use crate::frontend::app::state::{Selection, Studio, studio};
use crate::frontend::panels::scene::accent_css;
use crate::frontend::tasks::single_flight;

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
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();
    let focused = studio.focused();

    rsx! {
        div { class: "flex flex-col",
            ul { class: "flex flex-col",
                for (index, robot) in robots.iter().cloned().enumerate() {
                    RobotEntry {
                        key: "{robot.id}",
                        focused: focused.as_ref().is_some_and(|chosen| chosen.id == robot.id),
                        accent: accent_css(index),
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
fn RobotEntry(robot: RobotSummary, focused: bool, accent: &'static str) -> Element {
    let studio = studio();
    let mut chosen = studio.selected;
    // Open on whatever still needs attention. An arm that is wired up and
    // calibrated has nothing to do here, so it starts folded; anything else
    // opens on the work. Only the initial state is derived — an arm becoming
    // ready while the operator is reading it must not fold itself away.
    let ready = robot.problem.is_none() && robot.port.is_some() && robot.calibrated;
    let mut open = use_signal(|| !ready || !robot.connection.connected);
    let id = robot.id.clone();
    let role = robot_role(robot.robot_type.as_deref());
    let state = if robot.problem.is_some() {
        "Needs attention"
    } else if robot.port.is_none() {
        "Port needed"
    } else if !robot.calibrated {
        "Calibration needed"
    } else if robot.connection.connected {
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
                    aria_label: if open() { "Hide setup for {robot.id}" } else { "Show setup for {robot.id}" },
                    title: if open() { "Hide this arm's setup" } else { "Show this arm's setup" },
                    onclick: move |_| open.toggle(),
                    ChevronIcon { open: open() }
                }
                Action {
                    purpose: ActionPurpose::Select,
                    variant: ActionVariant::Row,
                    class: "device-main",
                    aria_pressed: focused,
                    title: "Show {robot.id} in the joints panel",
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
                        strong { "{robot.id}" }
                        span { "{role}" }
                    }
                    span { class: if ready { "device-state" } else { "device-state device-state-needed" }, "{state}" }
                    span {
                        class: "device-connection-dot",
                        style: if robot.connection.connected { "background: var(--tone-good)" } else { "background: var(--tone-line)" },
                        title: if robot.connection.connected { "Connected" } else { "Not connected" },
                    }
                }
            }
            if open() {
                RobotSetup { robot: robot.clone() }
            }
        }
    }
}

/// Adding an arm, kept folded away so the list stays a list.
#[component]
fn NewRobotForm() -> Element {
    let studio = studio();
    let mut chosen = studio.selected;
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
        let request = CreateRobotRequest {
            id: name.clone(),
            robot_type: Some(robot_type()),
            ..CreateRobotRequest::default()
        };
        let selected_name = name.clone();
        studio.run_utility(format!("Created {name}."), async move {
            let Some(result) = single_flight(
                creating,
                api::send(Method::POST, "/api/robots", Some(&request)),
            )
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
    let studio = studio();
    let importing = use_signal(|| false);
    let mut chosen = studio.selected;

    rsx! {
        FileAction {
            purpose: ActionPurpose::Import,
            variant: ActionVariant::Row,
            class: "device-list-action",
            title: "Import a unified Spiral Robotics JSON configuration",
            accept: ".json,application/json",
            disabled: importing(),
            aria_busy: importing(),
            onchange: move |event: Event<FormData>| {
                let Some(file) = event.files().first().cloned() else { return };
                studio.run_utility("", async move {
                    let Some(result) = single_flight(importing, async {
                            let bytes = file
                                .read_bytes()
                                .await
                                .map_err(|error| format!("Could not read {}: {error}", file.name()))?;
                            api::upload_json::<RobotSummary>(
                                "/api/robots/import",
                                &bytes,
                                "application/json",
                            )
                            .await
                        })
                        .await
                    else {
                        return Ok(());
                    };
                    match result {
                        Ok(robot) => {
                            chosen.set(Some(Selection::Robot(robot.id.clone())));
                            studio.report_success(format!("Imported {}.", robot.id));
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                });
            },
            if importing() { "Importing…" } else { "Import configuration" }
        }
    }
}

/// Everything about one arm stored in its SpiralDB configuration row.
#[component]
fn RobotSetup(robot: RobotSummary) -> Element {
    let studio = studio();
    let telemetry = studio.robot_telemetry(&robot.id);
    let progress = telemetry.and_then(|robot| robot.calibration);
    let connected = robot.connection.connected;
    // One scan shared by the picker and the identify workflow, so they can
    // never disagree about which ports exist or which are already taken.
    let mut ports =
        use_resource(|| async { api::get::<Vec<SerialPortSummary>>("/api/ports").await });
    let scan = ports.read_unchecked().clone();

    rsx! {
        div { class: "device-setup",
            // The connection used to get a row of its own reading "Motor bus
            // connected" above a section headed "Motor bus" — the same two words
            // twice, plus a dot and a state the arm's own row already carries in
            // its readiness label and its connection dot. Connect now sits
            // beside the port it opens, which is also the order the work happens
            // in.
            if let Some(problem) = &robot.problem {
                div { class: "device-setup-alarm",
                    p { "This arm's configuration needs attention before it can connect." }
                    details { class: "technical-details",
                        summary { "Technical details" }
                        code { "{problem}" }
                    }
                }
            }
            if let Some(error) = &robot.connection.error {
                {
                    let message = friendly_connection_error(error);
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
                    scan,
                    sweeping: progress.is_some(),
                    on_rescan: move |()| ports.restart(),
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
                ActionLink {
                    purpose: ActionPurpose::Export,
                    variant: ActionVariant::Link,
                    href: "/api/robots/{robot.id}/configuration",
                    download: "{robot.id}.json",
                    title: "Download the complete configuration for {robot.id}",
                    "Export"
                }
                DeleteRobotAction { robot: robot.clone() }
            }
        }
    }
}

fn robot_role(robot_type: Option<&str>) -> &'static str {
    match robot_type {
        Some(kind) if kind.contains("leader") => "Leader · hand-guided",
        Some(kind) if kind.contains("follower") => "Follower · driven",
        _ => "Robot arm",
    }
}

fn friendly_connection_error(error: &str) -> &'static str {
    let lowered = error.to_ascii_lowercase();
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

/// The result of the shared serial-port scan: still running, failed, or done.
type PortScan = Option<Result<Vec<SerialPortSummary>, String>>;

#[component]
fn ConnectionAction(robot: RobotSummary, sweeping: bool) -> Element {
    let studio = studio();
    let pending = use_signal(|| false);
    let id = robot.id.clone();
    let connected = robot.connection.connected;
    // Disconnecting mid-sweep throws the sweep away, so make the operator
    // finish or cancel deliberately rather than lose it to one click.
    let blocked = robot.port.is_none() || robot.problem.is_some() || (connected && sweeping);
    let reason = if robot.port.is_none() {
        "Assign a serial port first"
    } else if robot.problem.is_some() {
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
                let action = if connected { "disconnect" } else { "connect" };
                studio.run("", async move {
                    single_flight(pending, api::post(&format!("/api/robots/{id}/{action}")))
                        .await
                        .unwrap_or(Ok(()))
                });
            },
            {if pending() { if connected { "Disconnecting…" } else { "Connecting…" } } else if connected { "Disconnect" } else { "Connect" }}
        }
    }
}

#[component]
fn PortSection(
    robot: RobotSummary,
    scan: PortScan,
    sweeping: bool,
    on_rescan: EventHandler<()>,
) -> Element {
    let studio = studio();
    let mut identify_step = use_signal(|| IdentifyStep::Idle);
    let starting_identify = use_signal(|| false);
    let current = robot.port.clone().map(|port| port.path).unwrap_or_default();
    let baud = robot.port.as_ref().map_or(1_000_000, |port| port.baud_rate);
    let id = robot.id.clone();
    // The port belongs to the open bus once connected; changing it would make
    // the durable configuration disagree with the hardware, so the server refuses it too.
    let locked = robot.connection.connected;
    let scanning = scan.is_none();
    let found = match &scan {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    // A port recorded earlier but missing from the scan still has to appear,
    // otherwise the field would silently look unset.
    let missing = !current.is_empty() && !found.iter().any(|port| port.path == current);

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
                            let Some(result) = single_flight(
                                starting_identify,
                                api::get::<Vec<SerialPortSummary>>("/api/ports"),
                            )
                            .await
                            else {
                                return;
                            };
                            match result {
                                Ok(list) => identify_step.set(IdentifyStep::AwaitingUnplug(
                                    list.into_iter().map(|port| port.path).collect(),
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
                                assign_port(studio, id.clone(), baud, value);
                            }
                        }
                    },
                    option { value: "", selected: current.is_empty(), "Choose a serial port…" }
                    for port in found {
                        PortOption {
                            key: "{port.path}",
                            selected: port.path == current,
                            port,
                            owner: robot.id.clone(),
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
                    move |path: String| assign_port(studio, id.clone(), baud, path)
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
    let holder = port.used_by.as_ref().filter(|holder| **holder != owner);
    let mut label = port.path.clone();
    if let Some(description) = &port.description {
        label.push_str(" — ");
        label.push_str(description);
    }
    if let Some(holder) = holder {
        label.push_str(&format!(" (in use by {holder})"));
    }

    rsx! {
        option { value: "{port.path}", disabled: holder.is_some(), selected, "{label}" }
    }
}

/// Record a serial port on a robot, keeping its existing baud rate.
fn assign_port(studio: Studio, id: String, baud_rate: u32, path: String) {
    let request = UpdateRobotRequest {
        port: Some(PortSettings {
            path: path.clone(),
            baud_rate,
        }),
        ..UpdateRobotRequest::default()
    };
    studio.run(format!("Assigned {path}."), async move {
        api::send(Method::PATCH, &format!("/api/robots/{id}"), Some(&request)).await
    });
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
    let studio = studio();
    let mut step = step;
    let checking = use_signal(|| false);

    match step().clone() {
        IdentifyStep::Idle => rsx! {},
        IdentifyStep::AwaitingUnplug(before) => rsx! {
            div { class: "mt-1 rounded border border-line bg-sunken p-1.5 text-[11px]",
                p { class: "mb-1",
                    "Unplug the USB cable from "
                    strong { class: "font-medium", "{robot.id}" }
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
                            let request = IdentifyPortRequest { before: before.clone() };
                            spawn(async move {
                                let Some(result) = single_flight(
                                    checking,
                                    api::send_json::<_, IdentifyPortResponse>(
                                        Method::POST, "/api/ports/identify", Some(&request)
                                    ),
                                )
                                .await
                                else {
                                    return;
                                };
                                match result {
                                    Ok(response) => match response.port {
                                        Some(port) => {
                                            on_found.call(port.clone());
                                            step.set(IdentifyStep::Finished(response.message));
                                        }
                                        None => studio.report(response.message),
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
    let studio = studio();
    let available = studio.snapshot.read().cameras.clone();
    let mut role = use_signal(|| "wrist".to_owned());
    let mut device = use_signal(String::new);
    let saving_cameras = use_signal(|| false);
    let id = robot.id.clone();
    let chosen = available
        .iter()
        .find(|camera| camera.id == device())
        .cloned();

    let scanning = use_signal(|| false);
    let taken = robot
        .cameras
        .iter()
        .any(|camera| camera.name == role.read().trim());
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
                            if let Some(Err(error)) = single_flight(
                                scanning,
                                api::post("/api/cameras/refresh"),
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
            // the same treatment an episode card already gives its delete. It
            // stays visible on touch, where there is no hover to reveal it.
            ul { class: "mb-1 flex flex-col gap-0.5",
                for binding in robot.cameras.clone() {
                    li { key: "{binding.name}", class: "device-camera-row ui-hover-action-host",
                        span { class: "shrink-0", "{binding.name}" }
                        span { class: "min-w-0 flex-1 truncate font-mono text-[10px] text-ink-faint",
                            "{binding.id}"
                        }
                        Action {
                            purpose: ActionPurpose::Remove,
                            variant: ActionVariant::Icon,
                            class: "device-camera-remove ui-hover-action",
                            disabled: saving_cameras(),
                            aria_busy: saving_cameras(),
                            aria_label: "Unassign the {binding.name} camera",
                            title: "Unassign the {binding.name} camera",
                            onclick: {
                                let id = id.clone();
                                let remaining = robot.cameras.iter()
                                    .filter(|camera| camera.name != binding.name)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                move |_| save_cameras(studio, id.clone(), remaining.clone(), saving_cameras)
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
                            key: "{camera.id}",
                            value: "{camera.id}",
                            selected: device() == camera.id,
                            "{camera.name} — {camera.width}×{camera.height} @ {camera.fps} fps"
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
                        src: api::camera_stream_url(&camera.id),
                        alt: "Live preview from {camera.name}",
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
                                let existing = robot.cameras.clone();
                                move |_| {
                                    let (name, device_id) = (role.read().trim().to_owned(), device());
                                    if name.is_empty() {
                                        studio.report("Give the camera a role name.");
                                        return;
                                    }
                                    let mut bindings = existing.iter()
                                        .filter(|camera| camera.name != name)
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    bindings.push(CameraBinding { name, id: device_id, mount: None });
                                    save_cameras(studio, id.clone(), bindings, saving_cameras);
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
fn save_cameras(studio: Studio, id: String, bindings: Vec<CameraBinding>, saving: Signal<bool>) {
    if saving.peek().to_owned() {
        return;
    }
    let request = UpdateRobotRequest {
        cameras: Some(bindings),
        ..UpdateRobotRequest::default()
    };
    studio.run("", async move {
        single_flight(
            saving,
            api::send(Method::PATCH, &format!("/api/robots/{id}"), Some(&request)),
        )
        .await
        .unwrap_or(Ok(()))
    });
}
