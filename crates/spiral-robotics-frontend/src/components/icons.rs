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

#[component]
pub(crate) fn TableNodeIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "2.5", y: "2.5", width: "11", height: "11", rx: "1" }
            path { d: "M2.5 6h11M6 6v7.5M10 6v7.5" }
        }
    }
}

#[component]
pub(crate) fn ModelNodeIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "m8 2.25 5 2.8v5.9l-5 2.8-5-2.8v-5.9l5-2.8Z" }
            path { d: "m3.25 5.2 4.75 2.7 4.75-2.7M8 7.9v5.55" }
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
