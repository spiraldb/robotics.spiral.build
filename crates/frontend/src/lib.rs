//! Browser-only Studio interface. Native compilation is used for unit tests, not SSR.
//!
//! The source tree follows the interface users see: product shell,
//! activities, panels, and shared components.

// Dioxus expands RSX interpolation such as `"{label}"` through `format!`, so
// this lint reports the macro's generated code rather than an avoidable call.
#![allow(clippy::useless_format)]

pub(crate) mod activities;
pub(crate) mod client;
mod components;
pub(crate) mod format;
pub(crate) mod panels;
pub(crate) mod shell;
pub(crate) mod tasks;

pub use shell::App;
