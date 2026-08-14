//! Spiral ADBC data preparation and backend-neutral training supervision.
//!
//! An injected training backend owns model and optimizer execution. Framework
//! tensors and backend-specific runtime types never enter Studio.

pub(crate) mod data;
pub(crate) mod manager;
pub(crate) mod remote;
