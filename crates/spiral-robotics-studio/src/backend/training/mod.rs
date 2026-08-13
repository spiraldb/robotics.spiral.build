//! Spiral ADBC data preparation and native Burn training supervision.
//!
//! The inference crate currently keeps Burn 0.21 Metal execution eager and
//! statically selected; its optional fusion and autotune planners both contain
//! first-step training failures on the full ACT workload.

pub(crate) const ACT_ARTIFACT_FORMAT: &str = "spiral_robotics.act.safetensors.v2";
pub(crate) const ACT_TENSOR_NAMESPACE: &str = "burn_act_state_dict_v1";

pub(crate) mod data;
pub(crate) mod manager;
