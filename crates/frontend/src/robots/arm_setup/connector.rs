//! Reaching the robot connector: how to start it, where this workspace looks
//! for it, the policy runtime it asks for actions, and whether it answers.

use dioxus::prelude::*;
use spiral_robotics_common::settings::DEFAULT_POLICY_RUNTIME;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, CopyIcon, Field, FieldLayout, Notice, NoticeTone,
    TextInput,
};

use crate::client::CommandError;
use crate::client::project::ProjectClient;
use crate::client::robot_connector::RobotConnector;

/// The command that starts the robot connector from a Studio checkout.
const START_COMMAND: &str = "cargo run -p spiral-robotics-connector-os";

/// Until the robot connector answers: the command that starts it. Always: the
/// address this workspace looks at, the policy runtime its rollouts ask for
/// actions, and what it heard there. Connecting or saving keeps both for
/// everyone in the workspace.
#[component]
pub(crate) fn RobotConnectorSetup() -> Element {
    let connector = RobotConnector::current();
    let mut address = use_signal(|| connector.address());
    let mut policy_runtime = use_signal(|| connector.policy_runtime());
    let mut saving = use_signal(|| false);
    let mut failure = use_signal(|| None::<CommandError>);
    // Each field moves with a save here or in another browser.
    use_effect(move || address.set(connector.address()));
    use_effect(move || policy_runtime.set(connector.policy_runtime()));
    let answering = connector.reachable();
    let address_changed = address.read().trim().trim_end_matches('/') != connector.address();
    let policy_runtime_changed =
        policy_runtime.read().trim().trim_end_matches('/') != connector.policy_runtime();
    let mut save = move |address: String, policy_runtime: String| {
        if *saving.peek() {
            return;
        }
        saving.set(true);
        failure.set(None);
        ProjectClient::current().spawn(async move {
            match connector.save_settings(&address, &policy_runtime).await {
                Ok(()) => connector.check_again(),
                Err(error) => failure.set(Some(error)),
            }
            saving.set(false);
        });
    };
    // Each row saves its own draft beside the other row's saved value.
    let mut connect = move || save(address.peek().clone(), connector.policy_runtime());
    let mut save_policy_runtime = move || save(connector.address(), policy_runtime.peek().clone());
    rsx! {
        div { class: "robot-connector-setup",
            if !answering {
                StartCommand {}
            }
            div { class: "robot-connector-endpoint",
                Field { label: "Address", layout: FieldLayout::Inline,
                    TextInput {
                        value: address(),
                        placeholder: "http://localhost:7070",
                        oninput: move |value| address.set(value),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Enter {
                                connect();
                            }
                        },
                    }
                }
                if address_changed || !answering {
                    Button {
                        purpose: ActionPurpose::Connect,
                        variant: ButtonVariant::Outline,
                        disabled: saving(),
                        aria_busy: saving(),
                        title: "Saves the address for everyone in this workspace",
                        onclick: move |_| connect(),
                        "Connect"
                    }
                }
            }
            div { class: "robot-connector-endpoint",
                Field { label: "Policy runtime", layout: FieldLayout::Inline,
                    TextInput {
                        value: policy_runtime(),
                        placeholder: DEFAULT_POLICY_RUNTIME,
                        title: "The robot connector asks it for actions, so localhost is the robot connector's machine",
                        oninput: move |value| policy_runtime.set(value),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Enter && policy_runtime_changed {
                                save_policy_runtime();
                            }
                        },
                    }
                }
                if policy_runtime_changed {
                    Button {
                        purpose: ActionPurpose::Apply,
                        variant: ButtonVariant::Outline,
                        disabled: saving(),
                        aria_busy: saving(),
                        title: "Saves the policy runtime for everyone in this workspace",
                        onclick: move |_| save_policy_runtime(),
                        "Save"
                    }
                }
            }
            if let Some(error) = failure() {
                Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
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
