//! Spiral ADBC data preparation and native Burn training supervision.
//!
//! The inference crate currently keeps Burn 0.21 Metal execution eager and
//! statically selected; its optional fusion and autotune planners both contain
//! first-step training failures on the full ACT workload.

pub(crate) mod data;
pub(crate) mod manager;
