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

/// A rail across the foot of the workspace.
#[component]
pub fn Rail() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "1.5", y: "2", width: "13", height: "12", rx: "2" }
            path { d: "M1.5 10.5h13" }
        }
    }
}

#[component]
pub fn Panel(left: bool) -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            rect { x: "1.5", y: "2", width: "13", height: "12", rx: "2" }
            if left {
                path { d: "M5.5 2v12" }
            } else {
                path { d: "M10.5 2v12" }
            }
        }
    }
}
