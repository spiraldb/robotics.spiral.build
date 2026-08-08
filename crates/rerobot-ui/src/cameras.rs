//! Camera feeds are first-class views in the central workspace.

use dioxus::prelude::*;
use rerobot_core::api::CameraStatus;

use crate::api;
use crate::icons::Expand;
use crate::state::{Selection, studio};

/// Live views from every camera assigned to the focused robot.
#[component]
pub fn CameraWorkspace() -> Element {
    let studio = studio();
    let Some(robot) = studio.focused() else {
        return rsx! {};
    };
    if robot.cameras.is_empty() {
        return rsx! {};
    }

    let known = studio.snapshot.read().cameras.clone();
    let mut maximized = use_signal(|| None::<String>);
    let robot_id = robot.id.clone();

    rsx! {
        section {
            class: if maximized.read().is_some() { "camera-workspace camera-workspace-maximized" } else { "camera-workspace" },
            "aria-label": "Camera views for {robot.id}",
            for binding in robot.cameras.clone() {
                if maximized.read().as_ref().is_none_or(|role| role == &binding.name) {
                    CameraView {
                        key: "{binding.name}",
                        robot: robot_id.clone(),
                        role: binding.name.clone(),
                        device: binding.id.clone(),
                        status: known.iter().find(|camera| camera.id == binding.id).cloned(),
                        maximized: maximized.read().as_ref() == Some(&binding.name),
                        on_maximize: {
                            let role = binding.name.clone();
                            move |_| {
                                if maximized.read().as_ref() == Some(&role) {
                                    maximized.set(None);
                                } else {
                                    maximized.set(Some(role.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CameraView(
    robot: String,
    role: String,
    device: String,
    status: Option<CameraStatus>,
    maximized: bool,
    on_maximize: EventHandler<()>,
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
            class: if selected { "camera-tile camera-tile-selected" } else { "camera-tile" },
            onclick: {
                let (robot, role) = (robot.clone(), role.clone());
                move |_| studio.selected.set(Some(Selection::Camera {
                    robot: robot.clone(),
                    role: role.clone(),
                }))
            },
            header { class: "camera-tile-header",
                span { class: "camera-tile-title", "{role}" }
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
                button {
                    r#type: "button",
                    class: "icon-button",
                    "aria-label": if maximized { "Restore camera grid" } else { "Maximize {role} camera" },
                    title: if maximized { "Restore camera grid" } else { "Maximize camera" },
                    onclick: move |event| {
                        event.stop_propagation();
                        on_maximize.call(());
                    },
                    Expand { contract: maximized }
                }
            }
            div { class: "camera-frame",
                if trouble.is_none() {
                    img {
                        class: "camera-image",
                        src: api::camera_stream_url(&device),
                        alt: "Live view from the {role} camera on {robot}",
                    }
                } else {
                    div { class: "camera-unavailable",
                        span { class: "camera-placeholder-icon", "×" }
                        span { "Camera unavailable" }
                    }
                }
            }
            if let Some(trouble) = trouble {
                p { class: "camera-trouble", "{trouble}" }
            } else if frames == 0 {
                p { class: "camera-trouble text-ink-faint", "Opened; waiting for the first frame." }
            }
        }
    }
}
