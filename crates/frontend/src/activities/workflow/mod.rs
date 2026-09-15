//! Relation-first workflow canvas and its session-owned frontend state.

pub(crate) mod state;
pub(crate) mod workspace;

mod client;
pub(crate) use client::WorkflowClient;

mod agent;
mod document_controls;
mod plan_display;

mod assistant_content;
mod highlight;

mod comparison;

mod function_discovery;
