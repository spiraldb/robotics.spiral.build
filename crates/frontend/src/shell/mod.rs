//! Frontend providers and shell-level ownership boundaries.

pub(crate) mod activity;
pub(crate) mod frame;
pub(crate) mod layout;
pub(crate) mod notifications;
pub(crate) mod project;
pub(crate) mod routes;
pub(crate) mod state;
pub(crate) mod view_state;
pub(crate) mod workspace;

use crate::components::interaction::UiStyles;
use dioxus::prelude::*;

use self::routes::StudioRouter;

/// Render the routed shell; its layout owns application-wide providers.
#[component]
pub fn App() -> Element {
    rsx! {
        UiStyles {}
        StudioRouter {}
    }
}

mod client;
pub(crate) use client::StudioClient;

mod shortcuts;
