//! Frontend providers and shell-level ownership boundaries.

pub(crate) mod activity;
pub(crate) mod frame;
pub(crate) mod notifications;
pub(crate) mod project;
pub(crate) mod routes;
mod sign_in;
pub(crate) mod state;
pub(crate) mod workspace;

use dioxus::prelude::*;
use spiral_robotics_ui::UiStyles;

use self::routes::StudioRouter;

/// Render the routed shell; its layout owns application-wide providers.
#[component]
pub fn App() -> Element {
    rsx! {
        UiStyles {}
        StudioRouter {}
    }
}
