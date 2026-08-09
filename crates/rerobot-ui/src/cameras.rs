//! Live camera properties of the selected robot instance.

use dioxus::prelude::*;
use rerobot_core::api::{CameraStatus, RobotSummary};

use crate::api;
use crate::icons::{CameraOff, Chevron};
use crate::state::{Selection, studio};

/// The selected robot's assigned camera feeds, grouped in its properties panel.
#[component]
pub fn RobotCameras(robot: RobotSummary) -> Element {
    let studio = studio();
    let known = studio.snapshot.read().cameras.clone();
    let mut open = use_signal(|| true);
    let count = robot.cameras.len();
    let region_id = format!("robot-cameras-{}", robot.id);

    rsx! {
        section { class: "camera-properties",
            button {
                r#type: "button",
                class: "property-section-toggle",
                "aria-expanded": open(),
                "aria-controls": "{region_id}",
                title: if open() { "Hide camera properties" } else { "Show camera properties" },
                onclick: move |_| open.toggle(),
                Chevron { open: open() }
                span { class: "property-section-title", "Cameras" }
                span { class: "property-section-count", "{count}" }
            }
            if open() {
                div { id: "{region_id}", class: "property-camera-list",
                    if robot.cameras.is_empty() {
                        p { class: "property-camera-empty",
                            "No cameras assigned. Add one in this robot's setup."
                        }
                    }
                    for binding in robot.cameras.clone() {
                        CameraProperty {
                            key: "{binding.name}",
                            robot: robot.id.clone(),
                            role: binding.name.clone(),
                            device: binding.id.clone(),
                            status: known.iter().find(|camera| camera.id == binding.id).cloned(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CameraProperty(
    robot: String,
    role: String,
    device: String,
    status: Option<CameraStatus>,
) -> Element {
    let mut studio = studio();
    let selected = studio
        .selected
        .read()
        .as_ref()
        .is_some_and(|selection| selection.matches_camera(&robot, &role));
    let trouble = match &status {
        None => Some("Not detected. Rescan devices or assign another camera.".to_owned()),
        Some(camera) => camera.error.clone(),
    };
    let (frames, resolution) = status.as_ref().map_or((0, None), |camera| {
        (
            camera.frames,
            Some(format!("{}×{}", camera.width, camera.height)),
        )
    });

    rsx! {
        article {
            class: if selected { "camera-property camera-property-selected" } else { "camera-property" },
            role: "button",
            tabindex: "0",
            "aria-label": "Select the {role} camera on {robot}",
            onclick: {
                let (robot, role) = (robot.clone(), role.clone());
                move |_| studio.selected.set(Some(Selection::Camera {
                    robot: robot.clone(),
                    role: role.clone(),
                }))
            },
            onkeydown: {
                let (robot, role) = (robot.clone(), role.clone());
                move |event| {
                    if matches!(event.key(), Key::Enter)
                        || matches!(event.key(), Key::Character(value) if value == " ")
                    {
                        event.prevent_default();
                        studio.selected.set(Some(Selection::Camera {
                            robot: robot.clone(),
                            role: role.clone(),
                        }));
                    }
                }
            },
            header { class: "camera-property-header",
                span { class: "camera-property-name", "{role}" }
                if trouble.is_none() {
                    span { class: "camera-live-state",
                        span { class: "status-dot" }
                        if frames > 0 { "Live" } else { "Opening" }
                    }
                } else {
                    span { class: "chip chip-danger", "Unavailable" }
                }
                if let Some(resolution) = resolution {
                    span { class: "camera-resolution", "{resolution}" }
                }
            }
            div { class: "camera-property-frame",
                if trouble.is_none() {
                    img {
                        class: "camera-image",
                        src: api::camera_stream_url(&device),
                        alt: "Live view from the {role} camera on {robot}",
                    }
                } else {
                    div { class: "camera-unavailable",
                        span { class: "camera-placeholder-icon", CameraOff {} }
                        span { "Camera unavailable" }
                    }
                }
            }
            // Only a real problem gets a line of its own. Waiting for the first
            // frame is already said by the header's "Opening", and saying it
            // twice added a row under every camera for a state that clears in
            // about a second.
            if let Some(trouble) = trouble {
                p { class: "camera-trouble", "{trouble}" }
            }
        }
    }
}
