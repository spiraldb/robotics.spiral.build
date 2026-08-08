//! The rerobot studio web interface.
//!
//! A client-rendered Dioxus application that talks to `rerobot-studio` over
//! JSON and server-sent events. It shares [`rerobot_core`] with the server, so
//! both ends agree on what a joint angle and a calibration are.

use wasm_bindgen::prelude::*;

mod api;
mod app;
mod armview;
mod cameras;
mod control;
mod setup;
mod state;

/// Mount the studio into the page.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook();
    dioxus::web::launch::launch(app::App, Vec::new(), Vec::new());
}

/// Send Rust panics to the browser console instead of losing them.
fn console_error_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
    }));
}
