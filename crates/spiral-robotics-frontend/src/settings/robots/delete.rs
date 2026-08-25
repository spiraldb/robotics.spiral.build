use crate::components::interaction::{Action, ActionPurpose, ActionVariant, InlineConfirmation};
use dioxus::prelude::*;
use gloo_net::http::Method;
use spiral_robotics_api::robots::RobotSummary;

use crate::client::http as api;
use crate::shell::state::studio;
use crate::tasks::single_flight;

/// Deleting an arm destroys its calibration, so it takes a specific second click.
#[component]
pub(super) fn DeleteRobotAction(robot: RobotSummary) -> Element {
    let studio = studio();
    let mut confirming = use_signal(|| false);
    let deleting = use_signal(|| false);
    let id = robot.id.clone();

    // Resting, this is the quietest thing in the file row: it is the rarest
    // action on the panel and the only destructive one, and it used to be the
    // boldest text in the row purely because it was a button and its neighbour
    // was not.
    if !confirming() {
        return rsx! {
            Action {
                id: "delete-robot-{robot.id}",
                purpose: ActionPurpose::Delete,
                variant: ActionVariant::Link,
                class: "device-delete", title: "Delete {robot.id}",
                onclick: move |_| confirming.set(true), "Delete"
            }
        };
    }

    rsx! {
        InlineConfirmation {
            id: "delete-robot-confirmation-{robot.id}",
            class: "device-delete-confirmation",
            message: format!(
                "Delete robot {}? Its setup and calibration will be permanently removed.",
                robot.id,
            ),
            confirm_label: if deleting() { "Deleting…" } else { "Delete robot" },
            cancel_label: "Keep robot",
            purpose: ActionPurpose::Delete,
            pending: deleting(),
            return_focus_id: "delete-robot-{robot.id}",
            on_confirm: move |_| {
                if deleting() {
                    return;
                }
                let id = id.clone();
                studio.run(format!("Deleted {id}."), async move {
                    let Some(result) = single_flight(
                        deleting,
                        api::send::<()>(Method::DELETE, &format!("/api/robots/{id}"), None),
                    )
                    .await
                    else {
                        return Ok(());
                    };
                    if result.is_ok() {
                        confirming.set(false);
                    }
                    result
                });
            },
            on_cancel: move |_| confirming.set(false),
        }
    }
}
