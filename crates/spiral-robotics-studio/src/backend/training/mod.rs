//! Spiral ADBC data preparation and native Burn training supervision.

pub(crate) const ACT_ARTIFACT_FORMAT: &str = "spiral_robotics.act.safetensors.v2";
pub(crate) const ACT_TENSOR_NAMESPACE: &str = "burn_act_state_dict_v1";

pub(crate) mod data;
pub(crate) mod manager;
