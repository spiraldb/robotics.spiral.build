//! Route-owned Training screen.

use dioxus::prelude::*;

use crate::frontend::screens::training::workspace::TrainingWorkspace;

#[component]
pub(crate) fn TrainingScreen() -> Element {
    rsx! { TrainingWorkspace {} }
}
