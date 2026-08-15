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
