//! Relation-first workflow canvas and its session-owned frontend state.

pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) use client::WorkflowClient;

mod agent;
mod document_controls;
mod plan_display;

mod assistant_content;

// Build compiles without a caller until the canvas wires it into the
// document bar; that wiring removes this allow.
#[allow(dead_code)]
mod build;

mod function_discovery;
