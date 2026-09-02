//! Backend-neutral training supervision.
//!
//! The injected worker owns data preparation, model, and optimizer execution.

pub(crate) mod execution;
pub(crate) mod manager;
pub(crate) mod remote;
pub(crate) mod runs;
pub(crate) mod service;
