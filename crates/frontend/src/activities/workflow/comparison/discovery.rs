//! Read-only evidence gathering shared by every recorded-signal comparison.
use crate::activities::{
    data::DatasetClient, workflow::function_discovery::NativeFunctionDiscovery,
};
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{datasets::DatasetSelection, relations::SpiralQuerySnapshot};

pub(super) struct SignalDiscovery;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetArguments {
    path: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArguments {
    sql: String,
}
impl SignalDiscovery {
    pub(super) fn tools() -> Vec<Value> {
        vec![
            NativeFunctionDiscovery::definition(),
            json!({"type":"function","function":{"name":"inspect_dataset","description":"Read dataset topics, schema names (not field types) and variant IDs when the supplied dataset_inspections do not answer a specific question. Reuse existing context whenever possible.","parameters":{"type":"object","properties":{"path":{"type":"array","items":{"type":"string"}}},"required":["path"],"additionalProperties":false}}}),
            json!({"type":"function","function":{"name":"inspect_query","description":"Resolve a specific missing field shape, unit or joint mapping with a read-only SQL sample. Returns schema and up to 3 rows. Batch independent probes; skip facts already supplied. Use SQL LIMIT to bound work and reuse recorded signals rather than decoding media or rerunning inference.","parameters":{"type":"object","properties":{"sql":{"type":"string"}},"required":["sql"],"additionalProperties":false}}}),
        ]
    }
    pub(super) async fn call(name: &str, arguments: &str) -> Result<Value, String> {
        match name {
            "inspect_functions" => {
                serde_json::from_str(&NativeFunctionDiscovery::inspect(arguments).await?)
                    .map_err(|error| error.to_string())
            }
            "inspect_dataset" => {
                let args: DatasetArguments =
                    serde_json::from_str(arguments).map_err(|error| error.to_string())?;
                let dataset = DatasetSelection::new(args.path)?;
                let inspection = DatasetClient::current()
                    .inspect_dataset(dataset)
                    .await
                    .map_err(|error| error.to_string())?;
                serde_json::to_value(inspection).map_err(|error| error.to_string())
            }
            "inspect_query" => {
                let args: QueryArguments =
                    serde_json::from_str(arguments).map_err(|error| error.to_string())?;
                let output = DatasetClient::current()
                    .run_spiral_query(args.sql, 3)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(Self::preview(&output))
            }
            _ => Err("Unknown discovery tool.".into()),
        }
    }
    fn preview(output: &SpiralQuerySnapshot) -> Value {
        json!({"columns":output.columns(),"sample":Self::safe_value(&json!(output.rows()),0),"truncated":output.truncated(),"notice":"Rows and nested arrays are bounded previews, never population counts."})
    }
    fn safe_value(value: &Value, depth: usize) -> Value {
        if depth > 10 {
            return Value::Null;
        }
        match value {
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .take(12)
                    .map(|value| Self::safe_value(value, depth + 1))
                    .collect(),
            ),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| key.as_str() != "data")
                    .map(|(key, value)| (key.clone(), Self::safe_value(value, depth + 1)))
                    .collect(),
            ),
            Value::String(text) if text.starts_with("data:") || text.len() > 1000 => {
                Value::String("[payload omitted]".into())
            }
            value => value.clone(),
        }
    }
}
