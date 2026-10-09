//! Linking the robot connector: how to start it, the token it links with, and
//! whether it is linked.

use dioxus::prelude::*;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, CopyIcon, InlineConfirmation, Notice, NoticeTone,
};

use crate::client::CommandError;
use crate::client::project::ProjectClient;
use crate::client::robot_connector::RobotConnector;

/// The command that starts the robot connector from a Studio checkout.
const START_COMMAND: &str = "cargo run -p spiral-robotics-connector-os";

/// Until a robot connector is linked, or while a fresh token is on screen:
/// the command that starts one. Always: whether one is linked, and the token
/// it links with, which is the workspace's, for everyone in it.
#[component]
pub(crate) fn RobotConnectorSetup() -> Element {
    // Studio shows a token only when it issues it, so this view holds it
    // until it closes.
    let mut fresh_token = use_signal(|| None::<String>);
    let linked = RobotConnector::current().linked();
    rsx! {
        div { class: "robot-connector-setup",
            if !linked || fresh_token.read().is_some() {
                StartCommand { token: fresh_token() }
            }
            RobotConnectorLink { on_issued: move |token| fresh_token.set(Some(token)) }
        }
    }
}

/// The start command for this Studio, copyable in one click. Without a
/// `token` it shows where one goes.
#[component]
fn StartCommand(token: Option<String>) -> Element {
    let studio = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    let token = token.as_deref().unwrap_or("<token>");
    let command = format!("{START_COMMAND} -- --studio {studio} --token {token}");
    // The command copied last: another token makes another command.
    let mut copied = use_signal(|| None::<String>);
    let is_copied = copied.read().as_deref() == Some(command.as_str());
    let copy = command.clone();
    rsx! {
        div { class: "robot-connector-command",
            code { "{command}" }
            Button {
                purpose: ActionPurpose::Copy,
                variant: ButtonVariant::Icon,
                aria_label: if is_copied { "Copied" } else { "Copy the command" },
                title: if is_copied { "Copied" } else { "Copy" },
                onclick: move |_| {
                    if let Some(window) = web_sys::window() {
                        let _ = window.navigator().clipboard().write_text(&copy);
                        copied.set(Some(copy.clone()));
                    }
                },
                CopyIcon {}
            }
        }
    }
}

/// Whether a robot connector is linked, beside the token that links one.
/// Replacing the token unlinks the robot connector using it, so that asks
/// first.
#[component]
fn RobotConnectorLink(on_issued: EventHandler<String>) -> Element {
    const REPLACE_ID: &str = "robot-connector-replace-token";
    let connector = RobotConnector::current();
    let mut issuing = use_signal(|| false);
    let mut confirming = use_signal(|| false);
    let mut failure = use_signal(|| None::<CommandError>);
    let mut issue = move || {
        if *issuing.peek() {
            return;
        }
        issuing.set(true);
        failure.set(None);
        let issued = connector.issue_token();
        ProjectClient::current().spawn(async move {
            match issued.await {
                Ok(token) => on_issued.call(token.token().to_owned()),
                Err(error) => failure.set(Some(error)),
            }
            issuing.set(false);
            confirming.set(false);
        });
    };
    let (tone, status) = match connector.health() {
        Some(Ok(health)) => (
            "good",
            format!("Linked · robot connector {}", health.version()),
        ),
        _ => ("waiting", "Waiting for a robot connector…".to_owned()),
    };
    rsx! {
        div { class: "robot-connector-link",
            p { class: "robot-connector-status", "data-tone": tone, role: "status",
                span { class: "robot-connector-status-dot", "aria-hidden": "true" }
                span { "{status}" }
            }
            if connector.token_issued() {
                Button {
                    id: REPLACE_ID,
                    purpose: ActionPurpose::Refresh,
                    variant: ButtonVariant::Link,
                    disabled: issuing(),
                    aria_expanded: confirming(),
                    onclick: move |_| confirming.toggle(),
                    "Replace token"
                }
            } else {
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Outline,
                    disabled: issuing(),
                    aria_busy: issuing(),
                    onclick: move |_| issue(),
                    "New token"
                }
            }
        }
        if confirming() {
            InlineConfirmation {
                id: "{REPLACE_ID}-confirmation",
                copy: ConfirmationCopy::new(
                    "Replace the token? The robot connector using it disconnects.",
                    "Replace token",
                    "Keep token",
                ),
                response: ConfirmationResponse::new(
                    ActionPurpose::Replace,
                    move |_| issue(),
                    move |_| confirming.set(false),
                ),
                dismissal: ConfirmationDismissal::new(REPLACE_ID).with_pending(issuing()),
            }
        }
        if let Some(error) = failure() {
            Notice { tone: NoticeTone::Danger, "{error} {error.recovery_guidance()}" }
        }
    }
}
