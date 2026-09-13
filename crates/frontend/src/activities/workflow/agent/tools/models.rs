use super::*;

pub(super) struct ListModelCapabilities;
impl WorkflowTool for ListModelCapabilities {
    fn name(&self) -> &'static str {
        "list_model_capabilities"
    }
    fn description(&self) -> &'static str {
        "Read the installed Spiral AI function catalog: signatures, effects, visibility, and descriptions. Optional model and device arguments use native defaults when omitted."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Checking model capabilities",
            "Model discovery",
            "check model support",
        )
    }
    fn available(&self, _workflow: &WorkflowSpec) -> bool {
        true
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion::count(output, "functions", "native AI functions")
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let _: NoArguments = invocation.arguments()?;
            let capabilities = WorkflowClient::current().capabilities().await?;
            Ok(
                json!({"functions": capabilities.rows(), "complete": !capabilities.truncated()})
                    .to_string(),
            )
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FunctionKind {
    Scalar,
    Aggregate,
    Relation,
}
impl FunctionKind {
    fn catalog(&self) -> &'static str {
        match self {
            Self::Scalar => "scalar_functions",
            Self::Aggregate => "aggregate_functions",
            Self::Relation => "table_functions",
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionArguments {
    kind: FunctionKind,
    prefix: String,
    #[serde(default)]
    signatures: bool,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

pub(super) struct InspectFunctions;
impl WorkflowTool for InspectFunctions {
    fn name(&self) -> &'static str {
        "inspect_functions"
    }
    fn description(&self) -> &'static str {
        "Discover installed native scalar, aggregate, or relation functions by name prefix, e.g. spql.video., spql.image., spql.mcap., spql.list. Returns names and documentation; request signatures=true with a specific function prefix for full native parameter types. Paginated, default 5 and maximum 10 entries. This reads the engine catalog without changing the workflow or running inference."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"kind":{"type":"string","enum":["scalar","aggregate","relation"]},"prefix":{"type":"string"},"signatures":{"type":"boolean"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":10}},"required":["kind","prefix"],"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Reading native function documentation",
            "Function discovery",
            "inspect native functions",
        )
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            use spiral_robotics_common::relations::{QueryRequest, SpiralQuerySnapshot};
            let args: FunctionArguments = invocation.arguments()?;
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
            let fields = if args.signatures {
                "*"
            } else {
                "function_set, overload, summary"
            };
            let sql = format!(
                "SELECT {fields} FROM spql.meta.{}() WHERE visibility = 'public' AND function_set LIKE '{}%' ORDER BY function_set, overload LIMIT {} OFFSET {}",
                args.kind.catalog(),
                args.prefix,
                limit + 1,
                args.offset
            );
            let result: SpiralQuerySnapshot = WorkflowClient::current()
                .query(
                    HttpRequest::new("POST", "/api/queries")
                        .json(&QueryRequest::new(sql, limit + 1)),
                )
                .await?;
            let rows = result.rows();
            Ok(json!({"functions": rows.iter().take(limit).collect::<Vec<_>>(), "next_offset": (rows.len() > limit).then_some(args.offset + limit)}).to_string())
        })
    }
}
