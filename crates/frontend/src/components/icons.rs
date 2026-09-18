//! Small, consistent line icons shared across frontend surfaces.

use dioxus::prelude::*;

/// The robot explorer: a compact arm silhouette rather than a generic sidebar.
#[component]
pub(crate) fn RobotPanelIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3 13h4M5 13V9.5l3-2.5 2.5-3M8 7l2.5 2.5M10.5 9.5l2-1.5M10.5 4l1.5-1.5" }
            circle { cx: "5", cy: "9.5", r: "1" }
            circle { cx: "8", cy: "7", r: "1" }
            circle { cx: "10.5", cy: "4", r: "1" }
        }
    }
}

/// Global configuration at the foot of the activity rail.
#[component]
pub(crate) fn SettingsIcon() -> Element {
    rsx! {
        // Sliders, not the circle-and-eight-rays this used to be. At 16px with a
        // 1.35 stroke those rays never resolved into gear teeth — they resolved
        // into a sun, which in a row of tool icons reads as brightness or
        // theme. Three tracks and three knobs say "the things you can set".
        // The tracks stop either side of each knob rather than running behind
        // it: every shape here is stroked, so an unbroken line would draw itself
        // straight through the middle of the circle sitting on it.
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 4.5h5.1M12.4 4.5h1.1M2.5 8h1.1M8.4 8h5.1M2.5 11.5h5.1M12.4 11.5h1.1" }
            circle { cx: "10", cy: "4.5", r: "1.5" }
            circle { cx: "6", cy: "8", r: "1.5" }
            circle { cx: "10", cy: "11.5", r: "1.5" }
        }
    }
}

/// Recorded dataset activity.
#[component]
pub(crate) fn DataIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            ellipse { cx: "8", cy: "3.5", rx: "5", ry: "2" }
            path { d: "M3 3.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4M3 7.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4" }
        }
    }
}

/// Relation lineage: two Spiral tables converging on a derived result.
#[component]
pub(crate) fn WorkflowIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            circle { cx: "3", cy: "4", r: "1.5" }
            circle { cx: "3", cy: "12", r: "1.5" }
            circle { cx: "9", cy: "8", r: "1.5" }
            circle { cx: "13.25", cy: "8", r: "1.25" }
            path { d: "M4.5 4.6 7.6 7M4.5 11.4 7.6 9M10.5 8h1.5" }
        }
    }
}

/// A contact sheet with one frame struck out: keep most, exclude some.
#[component]
pub(crate) fn CurationIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "2.5", width: "4.5", height: "4.5", rx: "0.75" }
            rect { x: "9", y: "2.5", width: "4.5", height: "4.5", rx: "0.75" }
            rect { x: "2.5", y: "9", width: "4.5", height: "4.5", rx: "0.75" }
            path { d: "M9.75 9.75l3 3M12.75 9.75l-3 3" }
        }
    }
}

#[component]
pub(crate) fn QueryNodeIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 3h11L9.25 8v4l-2.5 1V8L2.5 3Z" }
        }
    }
}

#[component]
pub(crate) fn ResultNodeIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "2.5", width: "11", height: "11", rx: "1" }
            path { d: "M2.5 6h11M2.5 9.75h11M6.25 6v7.5" }
        }
    }
}

#[component]
pub(crate) fn FitViewIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10" }
        }
    }
}

/// Policy training activity: a curve passing through optimization steps.
#[component]
pub(crate) fn TrainingIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 12.5h11M3.5 11l2.4-3 2.2 1.4 3.9-5" }
            circle { cx: "3.5", cy: "11", r: ".75" }
            circle { cx: "5.9", cy: "8", r: ".75" }
            circle { cx: "8.1", cy: "9.4", r: ".75" }
            circle { cx: "12", cy: "4.4", r: ".75" }
        }
    }
}

/// Policy rollout: a predicted action moving into the follower control path.
#[component]
pub(crate) fn RolloutIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3 2.75v10.5l8-5.25-8-5.25Z" }
            path { d: "M11.75 4.5h1.5v7h-1.5" }
        }
    }
}

/// Session messages live behind one VS Code-style status-bar affordance.
#[component]
pub(crate) fn NotificationIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.5 11.5h9l-1.25-1.75V7a3.25 3.25 0 0 0-6.5 0v2.75L3.5 11.5Z" }
            path { d: "M6.5 13a1.75 1.75 0 0 0 3 0" }
        }
    }
}

/// Native video and image stages: a film frame with sprocket marks.
#[component]
pub(crate) fn MediaStageIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "3.5", width: "11", height: "9", rx: "1" }
            path { d: "M5.25 3.5v9M10.75 3.5v9M2.5 6.5h2.75M2.5 9.5h2.75M10.75 6.5h2.75M10.75 9.5h2.75" }
        }
    }
}

/// Native AI inference: a four-point spark.
#[component]
pub(crate) fn ModelStageIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M7.5 2.75 8.9 6.6l3.85 1.4-3.85 1.4L7.5 13.25 6.1 9.4 2.25 8l3.85-1.4L7.5 2.75Z" }
            path { d: "M12.75 2.5v2M11.75 3.5h2" }
        }
    }
}

/// Two row populations combined on a key.
#[component]
pub(crate) fn JoinStageIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            circle { cx: "6", cy: "8", r: "3.75" }
            circle { cx: "10", cy: "8", r: "3.75" }
        }
    }
}

/// Start something: a run, a query.
#[component]
pub(crate) fn PlayIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4.75 3.25v9.5l7.5-4.75-7.5-4.75Z" }
        }
    }
}

/// Stop the current turn or run.
#[component]
pub(crate) fn StopIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "4", y: "4", width: "8", height: "8", rx: "1" }
        }
    }
}

/// Send a message: an upward arrow, the direction the thread grows.
#[component]
pub(crate) fn SendIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 12.75V3.5M4.25 7.25 8 3.5l3.75 3.75" }
        }
    }
}

/// Work in progress. The ring spins through `ui-icon--spin`.
#[component]
pub(crate) fn SpinnerIcon() -> Element {
    rsx! {
        svg { class: "ui-icon ui-icon--spin", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M13 8a5 5 0 1 1-1.9-3.93" }
        }
    }
}

#[component]
pub(crate) fn CheckIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.5 8.5 6.5 11.5 12.5 4.5" }
        }
    }
}

#[component]
pub(crate) fn ZoomInIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 3.75v8.5M3.75 8h8.5" }
        }
    }
}

#[component]
pub(crate) fn ZoomOutIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.75 8h8.5" }
        }
    }
}

/// Step to the previous row.
#[component]
pub(crate) fn ArrowUpIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 12.5v-9M4.5 7 8 3.5 11.5 7" }
        }
    }
}

/// Step to the next row.
#[component]
pub(crate) fn ArrowDownIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 3.5v9M4.5 9 8 12.5 11.5 9" }
        }
    }
}

/// The Assistant conversation.
#[component]
pub(crate) fn ConversationIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 4.5A1.5 1.5 0 0 1 4 3h8a1.5 1.5 0 0 1 1.5 1.5v5A1.5 1.5 0 0 1 12 11H7.5l-3 2.5V11H4a1.5 1.5 0 0 1-1.5-1.5v-5Z" }
        }
    }
}

/// The SQL definition.
#[component]
pub(crate) fn SqlIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M5.5 4 2.5 8l3 4M10.5 4l3 4-3 4M9 3 7 13" }
        }
    }
}

/// Stage details: a heading with two facts beneath it.
#[component]
pub(crate) fn DetailsIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3 4h10M3 8h6M3 12h8" }
        }
    }
}

/// Transport: hold the clock where it is.
#[component]
pub(crate) fn PauseIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M5 3.5v9M11 3.5v9" }
        }
    }
}

/// Transport: jump to the first message.
#[component]
pub(crate) fn SkipToStartIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4 3.5v9M12 3.5 6 8l6 4.5z" }
        }
    }
}

/// Transport: jump to the last message.
#[component]
pub(crate) fn SkipToEndIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M12 3.5v9M4 3.5 10 8l-6 4.5z" }
        }
    }
}

/// Transport: nudge the playhead backwards by one step.
#[component]
pub(crate) fn StepBackIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M10.5 4 6.5 8l4 4M5 4v8" }
        }
    }
}

/// Transport: nudge the playhead forwards by one step.
#[component]
pub(crate) fn StepForwardIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M5.5 4 9.5 8l-4 4M11 4v8" }
        }
    }
}

/// A recorded video stream.
#[component]
pub(crate) fn CameraStreamIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2", y: "4.5", width: "8.5", height: "7", rx: "1.2" }
            path { d: "m10.5 7 3.5-2v6l-3.5-2" }
        }
    }
}

/// A recorded data stream: samples over time.
#[component]
pub(crate) fn SignalStreamIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2 9.5h2.5l1.5-4 2 6 1.5-3.5 1 1.5H14" }
        }
    }
}

/// Fit the whole episode into the visible span.
#[component]
pub(crate) fn FitTimeIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M2.5 5v6M13.5 5v6M4 8h8M6 6 4 8l2 2M10 6l2 2-2 2" }
        }
    }
}
