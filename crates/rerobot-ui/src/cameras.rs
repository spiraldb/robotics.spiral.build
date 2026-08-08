//! Live camera previews.

use dioxus::prelude::*;
use rerobot_core::api::CameraStatus;

use crate::api;
use crate::state::studio;

#[component]
pub fn CamerasTab() -> Element {
    let studio = studio();
    let cameras = studio.snapshot.read().cameras.clone();
    let mut refreshing = use_signal(|| false);

    // Which robots use which camera, so a preview says what it is looking at.
    let assignments = studio
        .snapshot
        .read()
        .robots
        .iter()
        .flat_map(|robot| {
            robot
                .cameras
                .iter()
                .map(|camera| (camera.id.clone(), format!("{}: {}", robot.id, camera.name)))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "flex flex-wrap items-center gap-3",
                h2 { class: "text-base font-semibold", "Cameras" }
                p { class: "flex-1 text-sm text-slate-500 dark:text-slate-400",
                    "Streams start when a preview is on screen and stop shortly after it leaves."
                }
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    disabled: refreshing(),
                    onclick: move |_| {
                        refreshing.set(true);
                        spawn(async move {
                            if let Err(error) = api::post("/api/cameras/refresh").await {
                                studio.report(error);
                            }
                            refreshing.set(false);
                        });
                    },
                    {if refreshing() { "Scanning…" } else { "Rescan" }}
                }
            }

            if cameras.is_empty() {
                p { class: "card p-8 text-center text-sm text-slate-500 dark:text-slate-400",
                    "No cameras detected. Plug one in and rescan."
                }
            }
            div { class: "grid gap-4 md:grid-cols-2 xl:grid-cols-3",
                for camera in cameras {
                    CameraCard {
                        key: "{camera.id}",
                        assigned: assignments.iter()
                            .filter(|(id, _)| *id == camera.id)
                            .map(|(_, label)| label.clone())
                            .collect::<Vec<_>>(),
                        camera,
                    }
                }
            }
        }
    }
}

#[component]
fn CameraCard(camera: CameraStatus, assigned: Vec<String>) -> Element {
    rsx! {
        figure { class: "card overflow-hidden",
            div { class: "aspect-video bg-slate-900",
                img {
                    class: "h-full w-full object-contain",
                    src: api::camera_stream_url(&camera.id),
                    alt: "Live preview from {camera.name}",
                    loading: "lazy",
                }
            }
            figcaption { class: "flex flex-col gap-1 p-3",
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 truncate text-sm font-medium", "{camera.name}" }
                    span { class: "font-mono text-xs text-slate-400",
                        "{camera.width}×{camera.height} @ {camera.fps}"
                    }
                }
                p { class: "truncate font-mono text-xs text-slate-400", "{camera.id}" }
                if !assigned.is_empty() {
                    div { class: "flex flex-wrap gap-1",
                        for label in assigned {
                            span {
                                key: "{label}",
                                class: "chip bg-sky-100 text-sky-800 dark:bg-sky-500/15 dark:text-sky-300",
                                "{label}"
                            }
                        }
                    }
                }
                if let Some(error) = &camera.error {
                    p { class: "text-xs text-rose-600 dark:text-rose-400", "{error}" }
                } else if camera.streaming && camera.frames == 0 {
                    p { class: "text-xs text-slate-400",
                        "Opened, but no frame has arrived yet."
                    }
                }
            }
        }
    }
}
