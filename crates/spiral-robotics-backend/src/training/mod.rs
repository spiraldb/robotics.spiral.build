//! Backend-neutral training supervision.
//!
//! The injected worker owns data preparation, model, and optimizer execution.

pub mod execution;
pub(crate) mod manager;
pub(crate) mod remote;
pub mod runs;
pub mod service;
