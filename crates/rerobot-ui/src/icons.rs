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

#[component]
pub fn Expand(contract: bool) -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            if contract {
                path { d: "M2.5 6h3.5V2.5M13.5 10H10v3.5M6 6 2.5 2.5M10 10l3.5 3.5" }
            } else {
                path { d: "M6 2.5H2.5V6M10 13.5h3.5V10M2.5 2.5 6 6M13.5 13.5 10 10" }
            }
        }
    }
}
