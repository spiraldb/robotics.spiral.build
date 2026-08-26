//! Frontend providers and shell-level ownership boundaries.

pub(crate) mod activity;
pub(crate) mod frame;
pub(crate) mod layout;
mod navigation;
pub(crate) mod notifications;
pub(crate) mod routes;
pub(crate) mod state;
pub(crate) mod workspace;

use crate::components::interaction::UiStyles;
use dioxus::prelude::*;

use self::routes::StudioRouter;
use self::state::Studio;

/// Install application-wide providers once, then render the reactive shell.
#[component]
pub fn App() -> Element {
    Studio::use_studio();
    rsx! {
        UiStyles {}
        StudioRouter {}
    }
}
