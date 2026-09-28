//! The utility glyphs of the design language: what a control, a disclosure or
//! a transport draws when it needs a mark rather than a word. Domain icons
//! belong to the surface that means them — a robot arm, a curation sheet, a
//! stage kind — so those stay in the frontend and these stay small.

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

/// Work in progress. The ring spins through `ui-icon--spin`.
#[component]
pub fn SpinnerIcon() -> Element {
    rsx! {
        svg { class: "ui-icon ui-icon--spin", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M13 8a5 5 0 1 1-1.9-3.93" }
        }
    }
}

/// Done, confirmed, satisfied.
#[component]
pub fn CheckIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.5 8.5 6.5 11.5 12.5 4.5" }
        }
    }
}

/// Transport: start the run, the query, the playback.
#[component]
pub fn PlayIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4.75 3.25v9.5l7.5-4.75-7.5-4.75Z" }
        }
    }
}

/// Transport: hold where it is.
#[component]
pub fn PauseIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M5 3.5v9M11 3.5v9" }
        }
    }
}

/// Transport: jump to the beginning.
#[component]
pub fn SkipToStartIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M4 3.5v9M12 3.5 6 8l6 4.5z" }
        }
    }
}

/// Transport: jump to the end.
#[component]
pub fn SkipToEndIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M12 3.5v9M4 3.5 10 8l-6 4.5z" }
        }
    }
}

/// Transport: nudge one step back.
#[component]
pub fn StepBackIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M10.5 4 6.5 8l4 4M5 4v8" }
        }
    }
}

/// Transport: nudge one step forward.
#[component]
pub fn StepForwardIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M5.5 4 9.5 8l-4 4M11 4v8" }
        }
    }
}

/// Show more of it.
#[component]
pub fn ZoomInIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M8 3.75v8.5M3.75 8h8.5" }
        }
    }
}

/// Show less of it.
#[component]
pub fn ZoomOutIcon() -> Element {
    rsx! {
        svg { class: "ui-icon", view_box: "0 0 16 16", "aria-hidden": "true",
            path { d: "M3.75 8h8.5" }
        }
    }
}
