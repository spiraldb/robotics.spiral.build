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
