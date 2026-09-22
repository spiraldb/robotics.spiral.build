//! Backend-neutral training supervision.
//!
//! The injected worker owns data preparation, model, and optimizer execution.

pub(crate) mod http;
pub(crate) mod manager;
pub(crate) mod service;
