//! Reaching the robot connector: how to start it, where this browser looks
//! for it, and whether it answers.

use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, CopyIcon, Field, FieldLayout, TextInput,
};

use crate::client::robot_connector::RobotConnector;

/// The command that starts the robot connector from a Studio checkout.
const START_COMMAND: &str = "cargo run -p spiral-robotics-robot-connector";

/// Until the robot connector answers: the command that starts it. Always: the
/// address this browser looks at, and what it heard there.
#[component]
pub(crate) fn RobotConnectorSetup() -> Element {
    let connector = RobotConnector::current();
    let mut draft = use_signal(|| connector.address());
    let answering = connector.reachable();
    let changed = draft.read().trim().trim_end_matches('/') != connector.address();
    rsx! {
        div { class: "robot-connector-setup",
            if !answering {
                StartCommand {}
            }
            div { class: "robot-connector-address",
                Field { label: "Address", layout: FieldLayout::Inline,
                    TextInput {
                        value: draft(),
                        placeholder: "http://localhost:7070",
                        oninput: move |value| draft.set(value),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Enter {
                                connector.set_address(&draft.peek());
                                connector.check_again();
                            }
                        },
                    }
                }
                if changed || !answering {
                    Button {
                        purpose: ActionPurpose::Connect,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| {
                            connector.set_address(&draft.peek());
                            connector.check_again();
                        },
                        "Connect"
                    }
                }
            }
            RobotConnectorStatus {}
        }
    }
}

/// The start command, copyable in one click.
#[component]
fn StartCommand() -> Element {
    let mut copied = use_signal(|| false);
    rsx! {
        div { class: "robot-connector-command",
            code { "{START_COMMAND}" }
            Button {
                purpose: ActionPurpose::Copy,
                variant: ButtonVariant::Icon,
                aria_label: if copied() { "Copied" } else { "Copy the command" },
                title: if copied() { "Copied" } else { "Copy" },
                onclick: move |_| {
                    if let Some(window) = web_sys::window() {
                        let _ = window.navigator().clipboard().write_text(START_COMMAND);
                        copied.set(true);
                    }
                },
                CopyIcon {}
            }
        }
    }
}

/// What this browser last heard from the robot connector.
#[component]
fn RobotConnectorStatus() -> Element {
    let connector = RobotConnector::current();
    let address = connector.address();
    let place = address
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned();
    let (tone, message) = match connector.health() {
        Some(Ok(health)) => (
            "good",
            format!(
                "Connected to {place} · robot connector {}",
                health.version()
            ),
        ),
        Some(Err(_)) => ("waiting", format!("Waiting for {place}…")),
        None => ("waiting", format!("Looking for {place}…")),
    };
    rsx! {
        p { class: "robot-connector-status", "data-tone": tone, role: "status",
            span { class: "robot-connector-status-dot", "aria-hidden": "true" }
            span { "{message}" }
        }
        if tone == "waiting" && connector.needs_local_network_permission() {
            p { class: "robot-connector-permission",
                "Your browser may ask to let this page reach devices on your network. Allow it."
            }
        }
    }
}
