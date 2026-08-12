//! Route-owned Training screen.

use dioxus::prelude::*;

use crate::frontend::app::routes::studio_configuration;
use crate::frontend::screens::training::workspace::TrainingWorkspace;

#[component]
pub(crate) fn TrainingScreen() -> Element {
    let configuration = studio_configuration();
    rsx! { TrainingWorkspace { dataset: configuration.dataset } }
}
