//! Browser-only Studio interface, tested in a browser through wasm-bindgen-test.
//!
//! The source tree follows the interface users see: product shell,
//! activities, panels, and shared components.

// Dioxus expands RSX interpolation such as `"{label}"` through `format!`, so
// this lint reports the macro's generated code rather than an avoidable call.
#![allow(clippy::useless_format)]
// Native targets only run the filesystem-based source checks in tests/.
#![cfg(target_arch = "wasm32")]

#[cfg(test)]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

pub(crate) mod activities;
pub(crate) mod client;
mod components;
pub(crate) mod format;
pub(crate) mod panels;
pub(crate) mod shell;
pub(crate) mod tasks;

pub use shell::App;
