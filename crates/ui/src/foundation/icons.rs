//! The utility glyphs the primitives themselves draw. Domain icons belong to
//! the surface that means them; these stay small so the crate never becomes
//! an icon set.

use dioxus::prelude::*;

#[component]
pub fn ChevronIcon(open: bool) -> Element {
    rsx! {
        svg {
            class: if open { "ui-icon ui-icon--chevron-open" } else { "ui-icon" },
            view_box: "0 0 16 16",
            "aria-hidden": "true",
            path { d: "M6 3.5 10.5 8 6 12.5" }
        }
    }
}

#[component]
pub fn CloseIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4 4l8 8M12 4l-8 8" }
        }
    }
}

#[component]
pub fn DeleteIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.5 5.5h9M6 5.5V3.75h4v1.75M5 7.5v5M8 7.5v5M11 7.5v5M4.5 5.5l.5 8h6l.5-8" }
        }
    }
}

#[component]
pub fn RefreshIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M12.5 6A5 5 0 1 0 13 9.75" }
            path { d: "M12.5 2.75V6H9.25" }
        }
    }
}

/// Detach a reversible relationship without borrowing the delete metaphor.
#[component]
pub fn UnassignIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M6.25 10.75 5 12a2.5 2.5 0 0 1-3.5-3.5l2-2A2.5 2.5 0 0 1 7 6.45" }
            path { d: "m9.75 5.25 1.25-1.25a2.5 2.5 0 1 1 3.5 3.5l-2 2A2.5 2.5 0 0 1 9 9.55" }
            path { d: "m5.75 10.25 4.5-4.5M3 3l10 10" }
        }
    }
}

/// Step to the previous row.
#[component]
pub fn ArrowUpIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 12.5v-9M4.5 7 8 3.5 11.5 7" }
        }
    }
}

/// Step to the next row.
#[component]
pub fn ArrowDownIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 3.5v9M4.5 9 8 12.5 11.5 9" }
        }
    }
}

/// Six dots: the handle a row is picked up by.
#[component]
pub fn GripIcon() -> Element {
    rsx! {
        svg { class: "ui-icon ui-icon--grip", view_box: "0 0 16 16", "aria-hidden": "true",
            for (x, y) in [(5, 3), (11, 3), (5, 8), (11, 8), (5, 13), (11, 13)] {
                circle { cx: "{x}", cy: "{y}", r: "1.2" }
            }
        }
    }
}
