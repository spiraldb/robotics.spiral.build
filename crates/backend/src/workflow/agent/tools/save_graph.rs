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
        "Validate graph structure and native SQL/Model schemas, then update an Arrow workflow in memory. If schema planning fails, correct the reported error and submit again; the existing graph is preserved. Validation never reads source batches or runs inference. Only the user's Save button persists the graph and conversation. SQL sources have no incoming edges. SQL transforms reference connected inputs with {{node:id}}. A Model consumes one parent, reads image_column, and appends named SigLIP scores while preserving input columns. Connect the final output to Result. Query max_rows bounds the preview; use SQL filters or LIMIT to bound inference inputs. Run is separate; every run recomputes inference."
    }
    fn parameters(&self) -> Value {
        let feature_definition_schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "maxLength": 96,
                    "pattern": "^[a-z_][a-z0-9_]*$"
                },
                "description": {"type": "string", "maxLength": 240},
                "concept": {"type": "string", "maxLength": 80}

            },
            "required": ["name", "description", "concept"],
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
                                            "image_column": {"type": "string"},
                                            "features": {
                                                "type": "array",
                                                "minItems": 1,
                                                "maxItems": 32,
                                                "items": feature_definition_schema
                                            }
                                        },
                                        "required": ["id", "name", "kind", "image_column", "features"],
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
        ToolPresentation::new("Validating graph", "Graph update", "update the workflow")
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::graph_updated(output)
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
            invocation.update_graph(workflow).await
        })
    }
}
