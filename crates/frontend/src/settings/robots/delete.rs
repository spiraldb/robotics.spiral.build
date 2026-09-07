use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, InlineConfirmation,
};
use crate::settings::robots::RobotClient;
use dioxus::prelude::*;
use spiral_robotics_common::robots::RobotSummary;

use crate::shell::state::studio;
use crate::tasks::SingleFlight;

/// Deleting an arm destroys its calibration, so it takes a specific second click.
#[component]
pub(super) fn DeleteRobotAction(robot: RobotSummary) -> Element {
    let studio = studio();
    let mut confirming = use_signal(|| false);
    let deleting = use_signal(|| false);
    let id = robot.id().to_owned();

    // Resting, this is the quietest thing in the file row: it is the rarest
    // action on the panel and the only destructive one, and it used to be the
    // boldest text in the row purely because it was a button and its neighbour
    // was not.
    if !confirming() {
        return rsx! {
            Action {
                id: "delete-robot-{robot.id()}",
                purpose: ActionPurpose::Delete,
                variant: ActionVariant::Link,
                class: "device-delete", title: "Delete {robot.id()}",
                onclick: move |_| confirming.set(true), "Delete"
            }
        };
    }

    rsx! {
        InlineConfirmation {
            id: "delete-robot-confirmation-{robot.id()}",
            class: "device-delete-confirmation",
            copy: ConfirmationCopy::new(
                format!(
                    "Delete robot {}? Its setup and calibration will be permanently removed.",
                    robot.id(),
                ),
                if deleting() { "Deleting…" } else { "Delete robot" },
                "Keep robot",
            ),
            response: ConfirmationResponse::new(
                ActionPurpose::Delete,
                move |_| {
                    if deleting() {
                        return;
                    }
                    let id = id.clone();
                    studio.run(format!("Deleted {id}."), async move {
                        let Some(result) = SingleFlight::run(
                            deleting,
                            async move {
                                RobotClient::current().delete_robot(id)
                                    .await
                                    .map_err(|error| error.to_string())
                            },
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
                move |_| confirming.set(false),
            ),
            dismissal: ConfirmationDismissal::new(format!("delete-robot-{}", robot.id()))
                .with_pending(deleting()),
        }
    }
}
