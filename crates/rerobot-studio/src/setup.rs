//! Creating robot instances and getting them ready to move.
//!
//! One arm is one robot instance is one JSON file. Everything on this panel —
//! the port, the cameras, the calibration — ends up in that single file.
//!
//! The panel is a list of arms above the setup for whichever one is selected,
//! so adding an arm, wiring it up, and calibrating it all happen beside the
//! 3D view of it rather than on a screen of their own.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    CameraBinding, CameraStatus, CreateRobotRequest, IdentifyPortRequest, IdentifyPortResponse,
    PortSettings, RobotSummary, SerialPortSummary, TorqueRequest, UpdateRobotRequest,
};

use crate::api;
use crate::icons::{Chevron, Trash};
use crate::state::{Selection, Studio, studio};
use crate::viewport::accent_css;

mod calibration;
mod delete;
use calibration::CalibrationSection;
use delete::DeleteButton;

/// Robot types offered when creating an instance.
const ROBOT_TYPES: [(&str, &str); 2] = [
    ("so101_follower", "Follower (driven arm)"),
    ("so101_leader", "Leader (hand-guided arm)"),
];

#[component]
pub fn RobotPanel() -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();
    let workspace = studio.snapshot.read().workspace.clone();
    let workspace_name = workspace
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&workspace)
        .to_owned();
    let focused = studio.focused();

    rsx! {
        div { class: "flex flex-col",
            // No count badge: the list is directly underneath and two rows long
            // on the bench this is built for. The directory earns its place
            // because it is the only thing on the panel that says where these
            // files live.
            header { class: "device-panel-header",
                h2 { "Robots" }
                span {
                    class: "device-workspace",
                    title: "Robot files are read and written in {workspace}",
                    "{workspace_name}"
                }
            }

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
    let mut open = use_signal(|| !ready);
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
                class: if focused { "device-entry-row device-entry-selected" } else { "device-entry-row" },
                button {
                    r#type: "button",
                    class: "device-chevron",
                    "aria-expanded": open(),
                    "aria-label": if open() { "Hide setup for {robot.id}" } else { "Show setup for {robot.id}" },
                    title: if open() { "Hide this arm's setup" } else { "Show this arm's setup" },
                    onclick: move |_| open.toggle(),
                    Chevron { open: open() }
                }
                button {
                    r#type: "button",
                    class: "device-main",
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

    let create = move || {
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
            api::send(Method::POST, "/api/robots", Some(&request)).await?;
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
            button {
                r#type: "button",
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
                onchange: move |event| robot_type.set(event.value()),
                for (value, label) in ROBOT_TYPES {
                    option { key: "{value}", value: "{value}", selected: robot_type() == value, "{label}" }
                }
            }
            div { class: "flex gap-1",
                button {
                    r#type: "button",
                    class: "btn btn-primary",
                    onclick: move |_| create(),
                    "Add"
                }
                button {
                    r#type: "button",
                    class: "btn",
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
    let mut importing = use_signal(|| false);
    let mut chosen = studio.selected;

    rsx! {
        label {
            class: if importing() { "device-list-action device-list-action-busy" } else { "device-list-action" },
            title: "Import a unified rerobot JSON configuration",
            input {
                class: "sr-only",
                r#type: "file",
                accept: ".json,application/json",
                disabled: importing(),
                onchange: move |event| {
                    let Some(file) = event.files().first().cloned() else { return };
                    importing.set(true);
                    let mut notice = studio.notice;
                    studio.run_utility("", async move {
                        let result = async {
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
                        }
                        .await;
                        importing.set(false);
                        match result {
                            Ok(robot) => {
                                chosen.set(Some(Selection::Robot(robot.id.clone())));
                                notice.set(Some(crate::state::Notice::new(
                                    format!("Imported {}.", robot.id),
                                    false,
                                )));
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    });
                }
            }
            if importing() { "Importing…" } else { "Import configuration" }
        }
    }
}

/// Everything about one arm that ends up in its file.
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
                p { class: "device-setup-alarm", "This arm's file cannot be used: {problem}" }
            }
            if let Some(error) = &robot.connection.error {
                p { class: "device-setup-alarm", "{error}" }
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
                span { class: "min-w-0 flex-1 truncate font-mono text-[10px] text-ink-faint", "{robot.file}" }
                a {
                    class: "link",
                    href: "/api/robots/{robot.id}/configuration",
                    download: "{robot.id}.json",
                    title: "Download the complete configuration for {robot.id}",
                    "Export"
                }
                DeleteButton { robot: robot.clone() }
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

/// The result of the shared serial-port scan: still running, failed, or done.
type PortScan = Option<Result<Vec<SerialPortSummary>, String>>;

#[component]
fn ConnectButton(robot: RobotSummary, sweeping: bool) -> Element {
    let studio = studio();
    let id = robot.id.clone();
    let connected = robot.connection.connected;
    // Disconnecting mid-sweep throws the sweep away, so make the operator
    // finish or cancel deliberately rather than lose it to one click.
    let blocked = robot.port.is_none() || robot.problem.is_some() || (connected && sweeping);
    let reason = if robot.port.is_none() {
        "Assign a serial port first"
    } else if robot.problem.is_some() {
        "Fix this arm's file first"
    } else if connected && sweeping {
        "Finish or cancel calibration first"
    } else {
        ""
    };

    rsx! {
        button {
            r#type: "button",
            class: if connected { "btn btn-outline" } else { "btn btn-primary" },
            disabled: blocked,
            title: "{reason}",
            onclick: move |_| {
                let id = id.clone();
                let action = if connected { "disconnect" } else { "connect" };
                studio.run("", async move { api::post(&format!("/api/robots/{id}/{action}")).await });
            },
            {if connected { "Disconnect" } else { "Connect" }}
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
    let current = robot.port.clone().map(|port| port.path).unwrap_or_default();
    let baud = robot.port.as_ref().map_or(1_000_000, |port| port.baud_rate);
    let id = robot.id.clone();
    // The port belongs to the open bus once connected; changing it would make
    // the file disagree with the hardware, so the server refuses it too.
    let locked = robot.connection.connected;
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
                button {
                    r#type: "button",
                    class: "link ml-auto",
                    onclick: move |_| on_rescan.call(()),
                    "Rescan"
                }
                button {
                    r#type: "button",
                    class: "link",
                    disabled: locked,
                    title: if locked { "Disconnect this arm before changing its port" } else { "Find the port by unplugging the cable" },
                    onclick: move |_| {
                        identify_step.set(IdentifyStep::Idle);
                        if !studio.controls_available() {
                            studio.report("Controls are paused because telemetry is unavailable or delayed.");
                            return;
                        }
                        spawn(async move {
                            match api::get::<Vec<SerialPortSummary>>("/api/ports").await {
                                Ok(list) => identify_step.set(IdentifyStep::AwaitingUnplug(
                                    list.into_iter().map(|port| port.path).collect(),
                                )),
                                Err(error) => studio.report(error),
                            }
                        });
                    },
                    "Identify"
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
                ConnectButton { robot: robot.clone(), sweeping }
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
                    button {
                        r#type: "button",
                        class: "btn btn-primary",
                        onclick: move |_| {
                            if !studio.controls_available() {
                                studio.report("Controls are paused because telemetry is unavailable or delayed.");
                                return;
                            }
                            let request = IdentifyPortRequest { before: before.clone() };
                            spawn(async move {
                                match api::send_json::<_, IdentifyPortResponse>(
                                    Method::POST, "/api/ports/identify", Some(&request)
                                ).await {
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
                        "I unplugged it"
                    }
                    button {
                        r#type: "button",
                        class: "btn",
                        onclick: move |_| step.set(IdentifyStep::Idle),
                        "Cancel"
                    }
                }
            }
        },
        IdentifyStep::Finished(message) => rsx! {
            div { class: "mt-1 flex items-start gap-1.5 rounded border border-line bg-sunken p-1.5 text-[11px]",
                span { class: "flex-1", "{message}" }
                button {
                    r#type: "button",
                    class: "link",
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
    let id = robot.id.clone();
    let chosen = available
        .iter()
        .find(|camera| camera.id == device())
        .cloned();

    let mut scanning = use_signal(|| false);
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
                button {
                    r#type: "button",
                    class: "link",
                    disabled: scanning(),
                    title: "Look for cameras that have been plugged in or moved",
                    onclick: move |_| {
                        if !studio.controls_available() {
                            studio.report("Controls are paused because telemetry is unavailable or delayed.");
                            return;
                        }
                        scanning.set(true);
                        spawn(async move {
                            if let Err(error) = api::post("/api/cameras/refresh").await {
                                studio.report(error);
                            }
                            scanning.set(false);
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
                    li { key: "{binding.name}", class: "device-camera-row",
                        span { class: "shrink-0", "{binding.name}" }
                        span { class: "min-w-0 flex-1 truncate font-mono text-[10px] text-ink-faint",
                            "{binding.id}"
                        }
                        button {
                            r#type: "button",
                            class: "device-camera-remove",
                            "aria-label": "Unassign the {binding.name} camera",
                            title: "Unassign the {binding.name} camera",
                            onclick: {
                                let id = id.clone();
                                let remaining = robot.cameras.iter()
                                    .filter(|camera| camera.name != binding.name)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                move |_| save_cameras(studio, id.clone(), remaining.clone())
                            },
                            Trash {}
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
                    onchange: move |event| device.set(event.value()),
                    option { value: "", selected: device().is_empty(), "Add a camera…" }
                    for camera in available.clone() {
                        CameraOption { key: "{camera.id}", selected: device() == camera.id, camera }
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
                            value: "{role}",
                            oninput: move |event| role.set(event.value()),
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-primary shrink-0",
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
                                    save_cameras(studio, id.clone(), bindings);
                                }
                            },
                            {if taken { "Replace" } else { "Add" }}
                        }
                    }
                }
            }
        }
    }
}

/// Replace a robot's camera assignments.
fn save_cameras(studio: Studio, id: String, bindings: Vec<CameraBinding>) {
    let request = UpdateRobotRequest {
        cameras: Some(bindings),
        ..UpdateRobotRequest::default()
    };
    studio.run("", async move {
        api::send(Method::PATCH, &format!("/api/robots/{id}"), Some(&request)).await
    });
}

#[component]
fn CameraOption(camera: CameraStatus, selected: bool) -> Element {
    rsx! {
        option { value: "{camera.id}", selected,
            "{camera.name} — {camera.width}×{camera.height} @ {camera.fps} fps"
        }
    }
}

/// Turn a torque toggle into a request, shared by the setup and joint panels.
pub fn set_torque(studio: Studio, id: &str, enabled: bool) {
    let id = id.to_owned();
    let request = TorqueRequest { enabled };
    studio.run("", async move {
        api::send(
            Method::POST,
            &format!("/api/robots/{id}/torque"),
            Some(&request),
        )
        .await
    });
}
