//! Native function discovery for workflow authoring.
use serde_json::{Value, json};
use spiral_robotics_common::relations::SpiralQuerySnapshot;
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FunctionKind {
    Scalar,
    Aggregate,
    Relation,
}

impl FunctionKind {
    fn catalog(&self) -> (&'static str, &'static str) {
        match self {
            Self::Scalar => ("scalar_functions", "scalar"),
            Self::Aggregate => ("aggregate_functions", "aggregate"),
            Self::Relation => ("table_functions", "table"),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DocumentedFunction {
    function_set: String,
    kind: String,
    summary: String,
    parameters: Vec<String>,
    returns: String,
    details: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionArguments {
    kind: Option<FunctionKind>,
    prefix: String,
    #[serde(default)]
    signatures: bool,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

impl FunctionArguments {
    fn sql(&self, limit: usize) -> String {
        let fields = if self.signatures {
            "function_set, overload, volatility, visibility, provider, effect, capabilities, parameters, summary"
        } else {
            "function_set, overload, summary"
        };
        let catalogs = self.kind.as_ref().map_or_else(
            || {
                vec![
                    FunctionKind::Scalar.catalog(),
                    FunctionKind::Aggregate.catalog(),
                    FunctionKind::Relation.catalog(),
                ]
            },
            |kind| vec![kind.catalog()],
        );
        let relations = catalogs.iter().map(|(catalog, kind)| format!(
            "SELECT {fields}, '{kind}' AS kind FROM spql.meta.{catalog}() WHERE visibility = 'public' AND function_set LIKE '{}%'",
            self.prefix
        )).collect::<Vec<_>>().join(" UNION ALL ");
        format!(
            "SELECT * FROM ({relations}) AS functions ORDER BY function_set, kind, overload LIMIT {} OFFSET {}",
            limit + 1,
            self.offset
        )
    }

    fn documented_functions(&self) -> Result<Vec<Value>, String> {
        // Source readers and generators have no entries in the live table-function
        // catalog. Keep this small, reviewed supplement separate from runtime facts.
        let functions: Vec<DocumentedFunction> =
            serde_json::from_str(include_str!("agent/tools/spiral_capabilities.json"))
                .map_err(|error| format!("Invalid documented Spiral capabilities: {error}"))?;
        Ok(functions
            .into_iter()
            .filter(|function| {
                function.function_set.starts_with(&self.prefix)
                    && match self.kind {
                        None => true,
                        Some(FunctionKind::Scalar) => function.kind == "scalar",
                        Some(FunctionKind::Aggregate) => function.kind == "aggregate",
                        Some(FunctionKind::Relation) => matches!(
                            function.kind.as_str(),
                            "source" | "generator" | "table"
                        ),
                    }
            })
            .map(|function| {
                if self.signatures {
                    json!(function)
                } else {
                    json!({"function_set": function.function_set, "kind": function.kind, "summary": function.summary})
                }
            })
            .collect())
    }

    fn response(&self, rows: &[serde_json::Map<String, Value>]) -> Result<Value, String> {
        let limit = self.limit.unwrap_or(5);
        Ok(json!({
            "functions": rows.iter().take(limit).collect::<Vec<_>>(),
            "next_offset": (rows.len() > limit).then_some(self.offset + limit),
            "documented_functions": self.documented_functions()?,
            "documentation_notice": "documented_functions is a curated Studio capability list, not runtime verification or an exhaustive inventory. All matching documented entries are included on every page; next_offset paginates only the live functions. Validate a candidate with set_workflow, then run and inspect a small query before relying on availability or results."
        }))
    }
}

pub(super) struct NativeFunctionDiscovery;
impl NativeFunctionDiscovery {
    pub(super) fn description() -> &'static str {
        "Discover Spiral functions by name prefix, e.g. spql.video., spql.image., spql.mcap., spql.list. Omit kind to search all supported categories. Returns live scalar/aggregate/table catalog entries plus a curated documented_functions list for source readers, generators, and supplemental scalar contracts; the relation filter includes sources and generators. Source readers introduce relations with FROM; generators expand input rows with CROSS JOIN LATERAL. Request signatures=true for parameter and output contracts before constructing calls. Documented capabilities are not proof of runtime availability. Live entries are paginated (default 5, maximum 10); all matching documented entries appear on every page. This reads metadata without running inference."
    }
    pub(super) fn parameters() -> Value {
        json!({"type":"object","properties":{"kind":{"type":"string","enum":["scalar","aggregate","relation"]},"prefix":{"type":"string"},"signatures":{"type":"boolean"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":10}},"required":["prefix"],"additionalProperties":false})
    }
    pub(super) async fn inspect(arguments: &str) -> Result<String, String> {
        let args: FunctionArguments =
            serde_json::from_str(arguments).map_err(|error| error.to_string())?;
        let limit = args.limit.unwrap_or(5);
        if !(1..=10).contains(&limit)
            || args.prefix.is_empty()
            || !args
                .prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Err("Provide a function-name prefix and a limit from 1 to 10.".into());
        }
        let result: SpiralQuerySnapshot = crate::activities::data::DatasetClient::current()
            .run_spiral_query(args.sql(limit), limit + 1)
            .await
            .map_err(|error| error.to_string())?;
        Ok(args.response(result.rows())?.to_string())
    }
}
#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn missing_live_entries_still_expose_video_contracts() {
        for (prefix, kind) in [
            ("spql.mcap.", None),
            ("spql.video.frames", Some("relation")),
        ] {
            let args: FunctionArguments =
                serde_json::from_value(json!({"prefix":prefix,"kind":kind,"signatures":true}))
                    .unwrap();
            let response = args.response(&[]).unwrap();
            assert_eq!(response["functions"], json!([]));
            assert_eq!(response["next_offset"], Value::Null);
            let functions = response["documented_functions"].as_array().unwrap();
            let name = if prefix == "spql.mcap." {
                "spql.mcap.read_video"
            } else {
                "spql.video.frames"
            };
            let function = functions
                .iter()
                .find(|row| row["function_set"] == name)
                .unwrap();
            assert!(function["parameters"].is_array());
            assert!(function["returns"].as_str().unwrap().contains("frame"));
            assert!(function["details"].is_string());
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn discovery_filters_and_paginates_live_catalogs_without_hiding_documentation() {
        let args: FunctionArguments =
            serde_json::from_value(json!({"prefix":"spql.","offset":10,"limit":2})).unwrap();
        let sql = args.sql(2);
        for catalog in ["scalar_functions", "aggregate_functions", "table_functions"] {
            assert!(sql.contains(&format!("spql.meta.{catalog}()")));
        }
        assert!(sql.ends_with("ORDER BY function_set, kind, overload LIMIT 3 OFFSET 10"));
        let response = args
            .response(&[
                serde_json::Map::new(),
                serde_json::Map::new(),
                serde_json::Map::new(),
            ])
            .unwrap();
        assert_eq!(response["functions"].as_array().unwrap().len(), 2);
        assert_eq!(response["next_offset"], 12);
        assert!(
            !response["documented_functions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            response["documented_functions"][0]
                .get("parameters")
                .is_none()
        );
        for kind in ["scalar", "aggregate"] {
            let args: FunctionArguments =
                serde_json::from_value(json!({"prefix":"spql.","kind":kind})).unwrap();
            let documented = args.documented_functions().unwrap();
            assert!(documented.iter().all(|function| function["kind"] == kind));
            assert!(!args.sql(5).contains("UNION ALL"));
        }
        let args: FunctionArguments =
            serde_json::from_value(json!({"prefix":"spql.not_a_function"})).unwrap();
        assert!(args.documented_functions().unwrap().is_empty());
        assert_eq!(
            NativeFunctionDiscovery::parameters()["required"],
            json!(["prefix"])
        );
    }
}
