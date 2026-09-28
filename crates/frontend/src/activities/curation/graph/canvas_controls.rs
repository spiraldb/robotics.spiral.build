//! Zoom and fit for the curation canvas: one hairline group at the bottom
//! left, quiet until needed.
use dioxus::prelude::*;
use dioxus_flow::use_flow;

use crate::components::icons::{FitViewIcon, ZoomInIcon, ZoomOutIcon};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

#[component]
pub(super) fn CanvasControls(label: String, fit_label: String) -> Element {
    let core = use_flow();
    rsx! {
        div { class: "curation-canvas-controls", role: "toolbar", "aria-label": "{label}",
            onpointerdown: move |event| event.stop_propagation(),
            Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                aria_label: "Zoom in", title: "Zoom in",
                onclick: move |_| core.zoom_in(200), ZoomInIcon {} }
            Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                aria_label: "Zoom out", title: "Zoom out",
                onclick: move |_| core.zoom_out(200), ZoomOutIcon {} }
            Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                aria_label: "{fit_label}", title: "{fit_label}",
                onclick: move |_| core.fit_view(400), FitViewIcon {} }
        }
    }
}
