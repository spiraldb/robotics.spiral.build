//! The Spiral Robotics Studio interface, compiled by DX for the browser and SSR.
//!
//! The native runtime lives in `spiral-robotics-studio-backend`, keeping UI
//! edits from invalidating robotics, storage, training, and inference code.

// Dioxus expands RSX interpolation such as `"{label}"` through `format!`, so
// this lint reports the macro's generated code rather than an avoidable call in
// Studio source.
#![allow(clippy::useless_format)]

mod frontend;

pub use frontend::App;
