//! Creating robot instances and getting them ready to move.
//!
//! One arm is one robot instance is one JSON file. Everything on this screen —
//! the port, the cameras, the calibration — ends up in that single file.

use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::{
    CalibrationProgress, CameraBinding, CameraStatus, CreateRobotRequest, IdentifyPortRequest,
    IdentifyPortResponse, PortSettings, RobotSummary, SerialPortSummary, TorqueRequest,
    UpdateRobotRequest,
};
use rerobot_core::arm;

use crate::api;
use crate::state::{Studio, studio};

/// Robot types offered when creating an instance.
const ROBOT_TYPES: [(&str, &str); 2] = [
    ("so101_follower", "Follower (driven arm)"),
    ("so101_leader", "Leader (hand-guided arm)"),
];

#[component]
pub fn SetupTab() -> Element {
    let studio = studio();
    let robots = studio.snapshot.read().robots.clone();
    let workspace = studio.snapshot.read().workspace.clone();

    rsx! {
        div { class: "flex flex-col gap-6",
            section { class: "card p-4",
                h2 { class: "mb-1 text-base font-semibold", "Robot instances" }
                p { class: "mb-4 text-sm text-slate-500 dark:text-slate-400",
                    "Each arm is stored as one JSON file in "
                    code { class: "rounded bg-slate-100 px-1 py-0.5 font-mono text-xs dark:bg-slate-800",
                        "{workspace}"
                    }
                }
                NewRobotForm {}
            }

            if robots.is_empty() {
                p { class: "card p-8 text-center text-sm text-slate-500 dark:text-slate-400",
                    "No robots yet. Add one above to assign its port, cameras, and calibration."
                }
            }
            for robot in robots {
                RobotCard { key: "{robot.id}", robot }
            }
        }
    }
}

#[component]
fn NewRobotForm() -> Element {
    let studio = studio();
    let mut id = use_signal(String::new);
    let mut robot_type = use_signal(|| ROBOT_TYPES[0].0.to_owned());

    let create = move |_| {
        let name = id.read().trim().to_owned();
        if name.is_empty() {
            studio.report("Give the robot a name first.");
            return;
        }
        let request = CreateRobotRequest {
            id: name.clone(),
            robot_type: Some(robot_type()),
            ..CreateRobotRequest::default()
        };
        id.set(String::new());
        studio.run(format!("Created {name}."), async move {
            api::send(Method::POST, "/api/robots", Some(&request)).await
        });
    };

    rsx! {
        div { class: "flex flex-wrap items-end gap-3",
            div { class: "min-w-48 flex-1",
                label { class: "label", r#for: "robot-id", "Name" }
                input {
                    id: "robot-id",
                    class: "field",
                    placeholder: "left_follower",
                    value: "{id}",
                    oninput: move |event| id.set(event.value()),
                }
            }
            div { class: "min-w-56 flex-1",
                label { class: "label", r#for: "robot-type", "Type" }
                select {
                    id: "robot-type",
                    class: "field",
                    value: "{robot_type}",
                    onchange: move |event| robot_type.set(event.value()),
                    for (value, label) in ROBOT_TYPES {
                        option { key: "{value}", value: "{value}", "{label}" }
                    }
                }
            }
            button { r#type: "button", class: "btn btn-primary", onclick: create, "Add robot" }
        }
    }
}

#[component]
fn RobotCard(robot: RobotSummary) -> Element {
    let studio = studio();
    let telemetry = studio.robot_telemetry(&robot.id);
    let calibrating = telemetry
        .as_ref()
        .and_then(|robot| robot.calibration.clone());
    let connected = robot.connection.connected;
    let id = robot.id.clone();

    rsx! {
        section { class: "card p-4",
            header { class: "mb-4 flex flex-wrap items-center gap-3",
                h3 { class: "text-base font-semibold", "{robot.id}" }
                span { class: "chip bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300",
                    {robot.robot_type.clone().unwrap_or_else(|| "unknown type".to_owned())}
                }
                if robot.calibrated {
                    span { class: "chip bg-emerald-100 text-emerald-800 dark:bg-emerald-500/15 dark:text-emerald-300",
                        "Calibrated"
                    }
                } else {
                    span { class: "chip bg-amber-100 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300",
                        "Not calibrated"
                    }
                }
                span { class: "font-mono text-xs text-slate-400", "{robot.file}" }

                div { class: "ml-auto flex items-center gap-2",
                    ConnectButton { robot: robot.clone() }
                    button {
                        r#type: "button",
                        class: "btn btn-ghost",
                        onclick: {
                            let id = id.clone();
                            move |_| {
                                let id = id.clone();
                                studio.run(format!("Deleted {id}."), async move {
                                    api::send::<()>(Method::DELETE, &format!("/api/robots/{id}"), None).await
                                });
                            }
                        },
                        "Delete"
                    }
                }
            }

            if let Some(error) = &robot.connection.error {
                p { class: "mb-4 rounded-md bg-rose-50 px-3 py-2 text-sm text-rose-800 dark:bg-rose-500/10 dark:text-rose-300",
                    "{error}"
                }
            }

            div { class: "grid gap-6 lg:grid-cols-2",
                PortSection { robot: robot.clone() }
                CameraSection { robot: robot.clone() }
            }

            div { class: "mt-6 border-t border-slate-200 pt-4 dark:border-slate-800",
                CalibrationSection {
                    robot: robot.clone(),
                    connected,
                    progress: calibrating,
                }
            }
        }
    }
}

#[component]
fn ConnectButton(robot: RobotSummary) -> Element {
    let studio = studio();
    let id = robot.id.clone();
    let connected = robot.connection.connected;

    rsx! {
        button {
            r#type: "button",
            class: if connected { "btn btn-ghost" } else { "btn btn-primary" },
            disabled: robot.port.is_none(),
            title: if robot.port.is_none() { "Assign a serial port first" } else { "" },
            onclick: move |_| {
                let id = id.clone();
                let action = if connected { "disconnect" } else { "connect" };
                studio.run("", async move {
                    api::post(&format!("/api/robots/{id}/{action}")).await
                });
            },
            {if connected { "Disconnect" } else { "Connect" }}
        }
    }
}

#[component]
fn PortSection(robot: RobotSummary) -> Element {
    let studio = studio();
    let mut ports =
        use_resource(|| async { api::get::<Vec<SerialPortSummary>>("/api/ports").await });
    let mut identify_step = use_signal(|| IdentifyStep::Idle);
    let current = robot.port.clone().map(|port| port.path).unwrap_or_default();
    let baud = robot.port.as_ref().map_or(1_000_000, |port| port.baud_rate);
    let id = robot.id.clone();

    rsx! {
        div {
            h4 { class: "label", "Motor bus" }
            div { class: "flex flex-wrap items-center gap-2",
                select {
                    class: "field flex-1",
                    disabled: robot.connection.connected,
                    value: "{current}",
                    onchange: {
                        let id = id.clone();
                        move |event: Event<FormData>| {
                            let value = event.value();
                            if !value.is_empty() {
                                assign_port(studio, id.clone(), baud, value);
                            }
                        }
                    },
                    option { value: "", "Choose a serial port…" }
                    match &*ports.read_unchecked() {
                        Some(Ok(list)) => rsx! {
                            for port in list.clone() {
                                option { key: "{port.path}", value: "{port.path}",
                                    "{port.path}"
                                    {port.description.as_ref().map(|text| format!(" — {text}")).unwrap_or_default()}
                                    {port.used_by.as_ref().filter(|owner| **owner != robot.id)
                                        .map(|owner| format!(" (in use by {owner})")).unwrap_or_default()}
                                }
                            }
                        },
                        Some(Err(error)) => rsx! { option { disabled: true, "{error}" } },
                        None => rsx! { option { disabled: true, "Scanning…" } },
                    }
                    // A port already recorded but no longer present still needs
                    // to be visible, otherwise the field would look unset.
                    if !current.is_empty()
                        && !matches!(&*ports.read_unchecked(), Some(Ok(list)) if list.iter().any(|port| port.path == current))
                    {
                        option { value: "{current}", "{current} (not connected)" }
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: move |_| ports.restart(),
                    "Rescan"
                }
            }

            IdentifyPort {
                robot: robot.clone(),
                step: identify_step,
                on_found: {
                    let id = id.clone();
                    move |path: String| assign_port(studio, id.clone(), baud, path)
                },
            }
            button {
                r#type: "button",
                class: "btn btn-ghost mt-2 w-full",
                disabled: robot.connection.connected,
                onclick: move |_| {
                    identify_step.set(IdentifyStep::Idle);
                    spawn(async move {
                        match api::get::<Vec<SerialPortSummary>>("/api/ports").await {
                            Ok(list) => identify_step.set(IdentifyStep::AwaitingUnplug(
                                list.into_iter().map(|port| port.path).collect(),
                            )),
                            Err(error) => studio.report(error),
                        }
                    });
                },
                "Identify by unplugging"
            }
        }
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
            div { class: "mt-2 rounded-md border border-sky-300 bg-sky-50 p-3 text-sm dark:border-sky-500/30 dark:bg-sky-500/10",
                p { class: "mb-2",
                    "Unplug the USB cable from "
                    strong { "{robot.id}" }
                    ", then continue. Leave every other device connected."
                }
                div { class: "flex gap-2",
                    button {
                        r#type: "button",
                        class: "btn btn-primary",
                        onclick: move |_| {
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
                        class: "btn btn-ghost",
                        onclick: move |_| step.set(IdentifyStep::Idle),
                        "Cancel"
                    }
                }
            }
        },
        IdentifyStep::Finished(message) => rsx! {
            div { class: "mt-2 flex items-start gap-3 rounded-md border border-emerald-300 bg-emerald-50 p-3 text-sm dark:border-emerald-500/30 dark:bg-emerald-500/10",
                span { class: "flex-1", "{message}" }
                button {
                    r#type: "button",
                    class: "cursor-pointer font-medium underline underline-offset-2",
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

    rsx! {
        div {
            h4 { class: "label", "Cameras" }
            if robot.cameras.is_empty() {
                p { class: "mb-2 text-sm text-slate-500 dark:text-slate-400", "None assigned." }
            }
            ul { class: "mb-2 flex flex-col gap-1",
                for binding in robot.cameras.clone() {
                    li {
                        key: "{binding.name}",
                        class: "flex items-center gap-2 rounded-md bg-slate-100 px-2.5 py-1.5 text-sm dark:bg-slate-800",
                        span { class: "font-medium", "{binding.name}" }
                        span { class: "flex-1 truncate font-mono text-xs text-slate-500 dark:text-slate-400",
                            "{binding.id}"
                        }
                        button {
                            r#type: "button",
                            class: "cursor-pointer text-xs font-medium text-rose-600 hover:underline dark:text-rose-400",
                            onclick: {
                                let id = id.clone();
                                let remaining = robot.cameras.iter()
                                    .filter(|camera| camera.name != binding.name)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                move |_| save_cameras(studio, id.clone(), remaining.clone())
                            },
                            "Remove"
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-end gap-2",
                div { class: "w-28",
                    input {
                        class: "field",
                        placeholder: "role",
                        value: "{role}",
                        oninput: move |event| role.set(event.value()),
                    }
                }
                select {
                    class: "field flex-1",
                    value: "{device}",
                    onchange: move |event| device.set(event.value()),
                    option { value: "", "Choose a camera…" }
                    for camera in available.clone() {
                        CameraOption { key: "{camera.id}", camera }
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: {
                        let id = id.clone();
                        let existing = robot.cameras.clone();
                        move |_| {
                            let (name, chosen) = (role.read().trim().to_owned(), device());
                            if name.is_empty() || chosen.is_empty() {
                                studio.report("Pick a camera and give it a role name.");
                                return;
                            }
                            let mut bindings = existing.iter()
                                .filter(|camera| camera.name != name)
                                .cloned()
                                .collect::<Vec<_>>();
                            bindings.push(CameraBinding { name, id: chosen });
                            device.set(String::new());
                            save_cameras(studio, id.clone(), bindings);
                        }
                    },
                    "Assign"
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
fn CameraOption(camera: CameraStatus) -> Element {
    rsx! {
        option { value: "{camera.id}",
            "{camera.name} — {camera.width}×{camera.height} @ {camera.fps} fps"
        }
    }
}

#[component]
fn CalibrationSection(
    robot: RobotSummary,
    connected: bool,
    progress: Option<CalibrationProgress>,
) -> Element {
    let studio = studio();
    let id = robot.id.clone();

    let Some(progress) = progress else {
        return rsx! {
            div { class: "flex flex-wrap items-center gap-3",
                div { class: "flex-1",
                    h4 { class: "label", "Calibration" }
                    p { class: "text-sm text-slate-500 dark:text-slate-400",
                        if robot.calibrated {
                            "Every joint has a recorded range. Recalibrate if the arm was rebuilt or a horn was moved."
                        } else {
                            "Record each joint's travel so the studio can command it safely."
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn-primary",
                    disabled: !connected,
                    title: if connected { "" } else { "Connect the robot first" },
                    onclick: move |_| {
                        let id = id.clone();
                        studio.run("", async move {
                            api::post(&format!("/api/robots/{id}/calibration/start")).await
                        });
                    },
                    {if robot.calibrated { "Recalibrate" } else { "Calibrate" }}
                }
            }
        };
    };

    rsx! {
        div {
            div { class: "mb-3 flex flex-wrap items-center gap-3",
                div { class: "flex-1",
                    h4 { class: "label", "Calibrating" }
                    p { class: "text-sm text-slate-600 dark:text-slate-300",
                        "Torque is off. Move every joint through its full range — to both ends — then save."
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn-primary",
                    disabled: !progress.ready,
                    title: if progress.ready { "" } else { "Every joint still needs to move" },
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            let id = id.clone();
                            studio.run("Calibration saved.", async move {
                                api::post(&format!("/api/robots/{id}/calibration/finish")).await
                            });
                        }
                    },
                    "Save calibration"
                }
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: {
                        let id = id.clone();
                        move |_| {
                            let id = id.clone();
                            studio.run("", async move {
                                api::post(&format!("/api/robots/{id}/calibration/cancel")).await
                            });
                        }
                    },
                    "Cancel"
                }
            }

            ul { class: "flex flex-col gap-1.5",
                for joint in progress.joints.clone() {
                    li { key: "{joint.name}", class: "flex items-center gap-3 text-sm",
                        span { class: "w-32 shrink-0 truncate",
                            {arm::so101_joint(&joint.name)
                                .map_or_else(|| joint.name.clone(), |known| known.label.to_owned())}
                        }
                        SweepBar { min: joint.min, position: joint.position, max: joint.max }
                        span { class: "w-16 shrink-0 text-right font-mono text-xs text-slate-500 dark:text-slate-400",
                            "{joint.span()}"
                        }
                        span {
                            class: if joint.span() >= 2 {
                                "w-4 shrink-0 text-emerald-600 dark:text-emerald-400"
                            } else {
                                "w-4 shrink-0 text-slate-300 dark:text-slate-600"
                            },
                            "✓"
                        }
                    }
                }
            }
        }
    }
}

/// A bar showing how much of a joint's travel has been swept so far.
#[component]
fn SweepBar(min: i64, position: i64, max: i64) -> Element {
    // The full encoder turn is the only fixed reference available before a
    // joint's real limits are known.
    let full = arm::ENCODER_COUNTS_PER_TURN as f64;
    let percent = |value: i64| (value as f64 / full * 100.0).clamp(0.0, 100.0);
    let left = percent(min);
    let width = (percent(max) - left).max(0.5);

    rsx! {
        div { class: "relative h-2 flex-1 rounded-full bg-slate-200 dark:bg-slate-700",
            div {
                class: "absolute inset-y-0 rounded-full bg-sky-500/70",
                style: "left: {left}%; width: {width}%",
            }
            div {
                class: "absolute top-1/2 h-3 w-1 -translate-x-1/2 -translate-y-1/2 rounded-full bg-slate-900 dark:bg-white",
                style: "left: {percent(position)}%",
            }
        }
    }
}

/// Turn a torque toggle into a request, shared by the setup and control tabs.
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
