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

    if !confirming() {
        return rsx! {
            button {
                r#type: "button", class: "btn", title: "Delete {robot.id}",
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
