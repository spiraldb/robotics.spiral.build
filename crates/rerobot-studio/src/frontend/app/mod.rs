//! Frontend providers and shell-level ownership boundaries.

pub(crate) mod activity;
mod layout;
mod navigation;
pub(crate) mod shell;
pub(crate) mod state;
mod workspace;

use dioxus::prelude::*;

use self::state::Studio;
use self::workspace::StudioApplication;

/// Install application-wide providers once, then render the reactive shell.
#[component]
pub fn App() -> Element {
    Studio::use_studio();
    state::use_description();
    rsx! { StudioApplication {} }
}
