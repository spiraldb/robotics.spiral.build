//! The rerobot studio web interface.
//!
//! A client-rendered Dioxus application that talks to `rerobot-studio` over
//! JSON and server-sent events. It shares [`rerobot_core`] with the server, so
//! both ends agree on what a joint angle and a calibration are.
//!
//! Build it with the Dioxus CLI, which also compiles `tailwind.css`:
//!
//! ```console
//! dx build --release --package rerobot-ui
//! ```

mod api;
mod app;
mod armview;
mod cameras;
mod control;
mod setup;
mod state;

fn main() {
    dioxus::launch(app::App);
}
