use dioxus::prelude::*;
use gloo_net::http::Method;
use rerobot_core::api::RobotSummary;

use crate::api;
use crate::state::studio;

/// Deleting an arm destroys its calibration, so it takes a specific second click.
#[component]
pub(super) fn DeleteButton(robot: RobotSummary) -> Element {
    let studio = studio();
    let mut confirming = use_signal(|| false);
    let id = robot.id.clone();

    // Resting, this is the quietest thing in the file row: it is the rarest
    // action on the panel and the only destructive one, and it used to be the
    // boldest text in the row purely because it was a button and its neighbour
    // was not.
    if !confirming() {
        return rsx! {
            button {
                r#type: "button", class: "link device-delete", title: "Delete {robot.id}",
                onclick: move |_| confirming.set(true), "Delete"
            }
        };
    }

    rsx! {
        button {
            r#type: "button", class: "btn btn-danger",
            title: "Delete {robot.id} and its calibration",
            onclick: move |_| {
                let id = id.clone();
                confirming.set(false);
                studio.run(format!("Deleted {id}."), async move {
                    api::send::<()>(Method::DELETE, &format!("/api/robots/{id}"), None).await
                });
            },
            "Delete {robot.id}"
        }
        button { r#type: "button", class: "btn", onclick: move |_| confirming.set(false), "Keep" }
    }
}
