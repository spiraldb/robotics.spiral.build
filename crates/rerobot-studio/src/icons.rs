//! Small, consistent line icons used by studio chrome.

use dioxus::prelude::*;

#[component]
pub fn Chevron(open: bool) -> Element {
    rsx! {
        svg {
            class: if open { "ui-icon chevron-open" } else { "ui-icon" },
            view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M6 3.5 10.5 8 6 12.5" }
        }
    }
}

/// The transport control's own mark: a disc to arm, a square to stop.
///
/// Filled rather than stroked, unlike the rest of these, because it is the one
/// control on the surface that starts capturing something.
#[component]
pub fn Transport(recording: bool) -> Element {
    rsx! {
        svg { class: "ui-icon ui-icon-solid", view_box: "0 0 16 16", "aria-hidden": "true",
            if recording {
                rect { x: "4.5", y: "4.5", width: "7", height: "7", rx: "1" }
            } else {
                circle { cx: "8", cy: "8", r: "4" }
            }
        }
    }
}

#[component]
pub fn Trash() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.5 5.5h9M6 5.5V3.75h4v1.75M5 7.5v5M8 7.5v5M11 7.5v5M4.5 5.5l.5 8h6l.5-8" }
        }
    }
}

/// The robot explorer: a compact arm silhouette rather than a generic sidebar.
#[component]
pub fn RobotPanelIcon() -> Element {
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
pub fn SettingsIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            circle { cx: "8", cy: "8", r: "2.25" }
            path { d: "M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.58 3.58l1.06 1.06M11.36 11.36l1.06 1.06M12.42 3.58l-1.06 1.06M4.64 11.36l-1.06 1.06" }
        }
    }
}

/// Recorded dataset activity.
#[component]
pub fn DataIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            ellipse { cx: "8", cy: "3.5", rx: "5", ry: "2" }
            path { d: "M3 3.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4M3 7.5v4c0 1.1 2.2 2 5 2s5-.9 5-2v-4" }
        }
    }
}
