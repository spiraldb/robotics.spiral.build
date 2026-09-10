use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWorkflowArguments {
    workflow: Value,
}

pub(super) struct SetWorkflow;
impl WorkflowTool for SetWorkflow {
    fn name(&self) -> &'static str {
        "set_workflow"
    }
    fn description(&self) -> &'static str {
        "Validate and save a complete Workflow graph. Provide its name, nodes, and edges; the server supplies identity and revision. A Query can be a SQL source with no incoming edges, or a transform referencing each connected input with {{node:id}}. A Model enriches one upstream candidate Query, and the final Query may join that cohort, the source-scoped feature cache, and other sources. Server-owned Model outputs are resolved automatically."
    }
    fn parameters(&self) -> Value {
        let feature_definition_schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "maxLength": 96,
                    "pattern": "^[a-z_][a-z0-9_]*_feature$"
                },
                "description": {"type": "string", "maxLength": 240},
                "concept": {"type": "string", "maxLength": 80},
                "applicability": {
                    "type": "object",
                    "properties": {
                        "source_relation": {"type": "string"},
                        "media_field": {"type": "string"},
                        "stream_ids": {
                            "type": "array",
                            "maxItems": 8,
                            "items": {"type": "string", "maxLength": 128}
                        }
                    },
                    "required": ["source_relation", "media_field"],
                    "additionalProperties": false
                }
            },
            "required": ["name", "description", "concept", "applicability"],
            "additionalProperties": false
        });

        json!({
            "type": "object",
            "properties": {
                "workflow": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "nodes": {
                            "type": "array",
                            "minItems": 2,
                            "items": {
                                "oneOf": [
                                    {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string"},
                                            "name": {"type": "string"},
                                            "kind": {"const": "table"},
                                            "relation": {"type": "string"}
                                        },
                                        "required": ["id", "name", "kind", "relation"],
                                        "additionalProperties": false
                                    },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string"},
                                            "name": {"type": "string"},
                                            "kind": {"const": "model"},
                                            "model_id": {"const": "siglip-base"},
                                            "features": {
                                                "type": "array",
                                                "minItems": 1,
                                                "maxItems": 32,
                                                "items": feature_definition_schema
                                            }
                                        },
                                        "required": ["id", "name", "kind", "model_id", "features"],
                                        "additionalProperties": false
                                    },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string"},
                                            "name": {"type": "string"},
                                            "kind": {"const": "query"},
                                            "sql": {"type": "string"},
                                            "max_rows": {"type": "integer", "minimum": 1, "maximum": 1000}
                                        },
                                        "required": ["id", "name", "kind", "sql", "max_rows"],
                                        "additionalProperties": false
                                    },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string"},
                                            "name": {"type": "string"},
                                            "kind": {"const": "result"}
                                        },
                                        "required": ["id", "name", "kind"],
                                        "additionalProperties": false
                                    }
                                ]
                            }
                        },
                        "edges": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "source": {"type": "string"},
                                    "target": {"type": "string"}
                                },
                                "required": ["source", "target"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["name", "nodes", "edges"],
                    "additionalProperties": false
                }
            },
            "required": ["workflow"],
            "additionalProperties": false
        })
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new("Validating graph", "Graph save", "save the workflow")
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::graph_saved(output)
    }
    fn call<'a>(&'a self, invocation: ToolInvocation<'a>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let arguments: SetWorkflowArguments = invocation.arguments()?;
            let mut value = arguments.workflow;
            let object = value
                .as_object_mut()
                .ok_or("set_workflow requires a workflow object.")?;
            // Identity and revision stay server-owned, outside the provider schema.
            object.insert("id".into(), json!(invocation.context.workflow_id));
            object.insert(
                "revision".into(),
                json!(invocation.context.workflow_revision),
            );
            let workflow: WorkflowSpec = serde_json::from_value(value)
                .map_err(|error| format!("Workflow shape is invalid: {error}"))?;
            invocation.save(workflow).await
        })
    }
}
