//! Native, serialization-only query diagnostics. These graphs never execute in Studio.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowPlan {
    report: Option<SpiralPlanReport>,
    error: Option<String>,
}
impl WorkflowPlan {
    pub(super) fn validate(&self) -> Result<(), String> {
        if let Some(report) = &self.report {
            report.validate()?;
        }
        Ok(())
    }
    pub fn from_native_json(json: &str) -> Result<Self, String> {
        let report: SpiralPlanReport = serde_json::from_str(json)
            .map_err(|error| format!("Reading Spiral's structured query plan: {error}"))?;
        report.validate()?;
        Ok(Self {
            report: Some(report),
            error: None,
        })
    }
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            report: None,
            error: Some(error.into()),
        }
    }
    pub fn error(&self) -> Option<&str> {
        self.error
            .as_deref()
            .or_else(|| self.report.as_ref()?.physical_lowering_error.as_deref())
    }
    pub fn report(&self) -> Option<&SpiralPlanReport> {
        self.report.as_ref()
    }
}

/// The native report is preserved, including text views and optional diagnostics.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SpiralPlanReport {
    explain_schema_version: u32,
    logical_plan: DiagnosticPlan,
    optimized_logical_plan: DiagnosticPlan,
    physical_plan: Option<DiagnosticPlan>,
    physical_lowering_error: Option<String>,
    #[serde(flatten)]
    diagnostics: Map<String, Value>,
}
impl SpiralPlanReport {
    fn validate(&self) -> Result<(), String> {
        if self.explain_schema_version != 2 {
            return Err("Studio requires Spiral EXPLAIN JSON schema version 2.".into());
        }
        self.logical_plan.validate()?;
        self.optimized_logical_plan.validate()?;
        if let Some(plan) = &self.physical_plan {
            plan.validate()?;
        }
        Ok(())
    }
    pub fn logical(&self) -> &DiagnosticPlan {
        &self.logical_plan
    }
    pub fn optimized(&self) -> &DiagnosticPlan {
        &self.optimized_logical_plan
    }
    pub fn physical(&self) -> Option<&DiagnosticPlan> {
        self.physical_plan.as_ref()
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DiagnosticPlan {
    schema_version: u32,
    root: u64,
    nodes: Vec<DiagnosticNode>,
}
impl DiagnosticPlan {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("Unsupported Spiral diagnostic graph version.".into());
        }
        let ids = self
            .nodes
            .iter()
            .map(DiagnosticNode::id)
            .collect::<BTreeSet<_>>();
        if ids.len() != self.nodes.len() || !ids.contains(&self.root) {
            return Err("Spiral's plan contains duplicate node IDs or a missing root.".into());
        }
        if self
            .nodes
            .iter()
            .flat_map(DiagnosticNode::inputs)
            .any(|input| !ids.contains(input))
        {
            return Err("Spiral's plan references a missing input node.".into());
        }
        Ok(())
    }
    pub fn root(&self) -> u64 {
        self.root
    }
    pub fn nodes(&self) -> &[DiagnosticNode] {
        &self.nodes
    }
    pub fn node(&self, id: u64) -> Option<&DiagnosticNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DiagnosticNode {
    id: u64,
    operator: String,
    inputs: Vec<u64>,
    output: Value,
    properties: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<Value>,
}
impl DiagnosticNode {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn operator(&self) -> &str {
        &self.operator
    }
    pub fn inputs(&self) -> &[u64] {
        &self.inputs
    }
    pub fn output(&self) -> &Value {
        &self.output
    }
    pub fn properties(&self) -> &Value {
        &self.properties
    }
    pub fn details(&self) -> Option<&str> {
        self.details.as_deref()
    }
    pub fn execution(&self) -> Option<&Value> {
        self.execution.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn report() -> Value {
        let graph = json!({"schema_version":1,"root":0,"nodes":[
            {"id":0,"operator":"Union","inputs":[1,1],"output":{},"properties":{"unknown_property":[1,2]}},
            {"id":1,"operator":"Values","inputs":[],"output":{},"properties":{}}
        ]});
        json!({"explain_schema_version":2,"logical_plan":graph,"optimized_logical_plan":graph,
            "physical_plan":null,"physical_lowering_error":"No physical implementation", "extra_diagnostic":{"value":42}})
    }
    #[test]
    fn diagnostic_reports_preserve_shared_inputs_properties_and_partial_plans() {
        let original = report();
        let plan = WorkflowPlan::from_native_json(&original.to_string()).unwrap();
        let report = plan.report().unwrap();
        assert_eq!(report.logical().node(0).unwrap().inputs(), &[1, 1]);
        assert_eq!(report.logical().nodes().len(), 2);
        assert_eq!(plan.error(), Some("No physical implementation"));
        assert!(report.physical().is_none());
        assert_eq!(serde_json::to_value(report).unwrap(), original);
    }
    #[test]
    fn unsupported_versions_and_broken_references_are_rejected() {
        let mut future = report();
        future["explain_schema_version"] = json!(3);
        let mut missing = report();
        missing["logical_plan"]["nodes"][0]["inputs"] = json!([9]);
        let mut duplicate = report();
        duplicate["logical_plan"]["nodes"][1]["id"] = json!(0);
        let mut text = report();
        text["logical_plan"] = json!("Project\n Values");
        for report in [future, missing, duplicate, text] {
            assert!(WorkflowPlan::from_native_json(&report.to_string()).is_err());
        }
    }
}
