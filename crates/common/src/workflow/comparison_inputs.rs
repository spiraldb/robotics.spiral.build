//! Ephemeral, complete workflow outputs reused by one comparison workspace.
use super::WorkflowSpec;
use crate::relations::SpiralQuerySnapshot;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonInputRequest {
    left: WorkflowSpec,
    right: Option<WorkflowSpec>,
}
impl ComparisonInputRequest {
    pub fn new(left: WorkflowSpec, right: Option<WorkflowSpec>) -> Self {
        Self { left, right }
    }
    pub fn left(&self) -> &WorkflowSpec {
        &self.left
    }
    pub fn right(&self) -> Option<&WorkflowSpec> {
        self.right.as_ref()
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CapturedComparisonInputs {
    id: String,
    left: SpiralQuerySnapshot,
    right: Option<SpiralQuerySnapshot>,
}
impl CapturedComparisonInputs {
    pub fn new(id: String, previews: (SpiralQuerySnapshot, Option<SpiralQuerySnapshot>)) -> Self {
        Self {
            id,
            left: previews.0,
            right: previews.1,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn left(&self) -> &SpiralQuerySnapshot {
        &self.left
    }
    pub fn right(&self) -> Option<&SpiralQuerySnapshot> {
        self.right.as_ref()
    }
}
