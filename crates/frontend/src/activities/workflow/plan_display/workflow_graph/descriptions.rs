use serde_json::Value;
use spiral_robotics_common::workflow::plan::DiagnosticNode;

use super::{
    Expression, ExpressionGraph, StageContent, StageFact, StageOutput, SummaryKind, WorkflowStage,
};

impl StageContent {
    pub(in super::super) fn relational(node: &DiagnosticNode) -> Self {
        let properties = &node.properties()["operator"];
        if node.operator() == "Source" {
            return Self::source(properties);
        }
        let (title, summary) = match node.operator() {
            "Values" => (
                "Input rows".into(),
                format!("{} literal rows", properties["rows"]),
            ),
            "Project" => (
                "Select columns".into(),
                properties["expressions"]
                    .as_array()
                    .map(|expressions| {
                        expressions
                            .iter()
                            .filter_map(|expr| expr["name"].as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ),
            "Filter" => (
                "Keep matching rows".into(),
                properties["predicate"].as_str().unwrap_or_default().into(),
            ),
            "Aggregate" => {
                let groups = Self::fields(&properties["group_by"]);
                let title = if groups.is_empty() {
                    "Summarize all rows".into()
                } else {
                    format!("Group by {groups}")
                };
                let aggregates = properties["aggregates"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|agg| {
                                let function = agg["function"].as_str().unwrap_or("aggregate");
                                let input = agg["input"]
                                    .as_str()
                                    .map(Self::field)
                                    .unwrap_or_else(|| "rows".into());
                                let name = agg["name"].as_str().unwrap_or("result");
                                let distinct = if agg["distinct"] == true {
                                    "distinct "
                                } else {
                                    ""
                                };
                                match function {
                                    "spiral.list.collect" => {
                                        format!("Collect {distinct}{input} values into {name}")
                                    }
                                    "vortex.min" => format!("Smallest {distinct}{input} → {name}"),
                                    "vortex.max" => format!("Largest {distinct}{input} → {name}"),
                                    "vortex.sum" => format!("Total {distinct}{input} → {name}"),
                                    "vortex.count" => format!("Count {distinct}{input} → {name}"),
                                    _ => format!(
                                        "{}({distinct}{input}) → {name}",
                                        function.rsplit('.').next().unwrap_or(function)
                                    ),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default();
                (title, aggregates)
            }
            "Window" => {
                let numbering = properties["expressions"]
                    .as_array()
                    .is_some_and(|expressions| {
                        !expressions.is_empty()
                            && expressions
                                .iter()
                                .all(|expr| expr["function"]["kind"] == "RowNumber")
                    });
                (
                    if numbering {
                        "Number rows within groups"
                    } else {
                        "Calculate across rows"
                    }
                    .into(),
                    Self::windows(properties),
                )
            }
            "Sort" => ("Order rows".into(), Self::ordering(&properties["keys"])),
            "TopK" => (
                "Select rows".into(),
                format!(
                    "{}; {}",
                    Self::limit(&properties["limit"], &properties["offset"]),
                    Self::ordering(&properties["keys"])
                ),
            ),
            "Limit" => (
                "Select rows".into(),
                Self::limit(&properties["fetch"], &properties["offset"]),
            ),
            "Distinct" => ("Remove duplicates".into(), Self::fields(&properties["by"])),
            "Join" => (
                format!("{} join", properties["kind"].as_str().unwrap_or("Row")),
                properties["on"]
                    .as_array()
                    .map(|keys| {
                        keys.iter()
                            .map(|key| {
                                format!(
                                    "{} = {}",
                                    Self::field(key["left"].as_str().unwrap_or_default()),
                                    Self::field(key["right"].as_str().unwrap_or_default())
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" and ")
                    })
                    .unwrap_or_default(),
            ),
            "TableFunction" | "Generate" => {
                let function = properties["function"].as_str().unwrap_or("Expand rows");
                let function = function.split_once('#').map_or(function, |(set, _)| set);
                let summary = if function == "spql.video.frames" {
                    let selection = ExpressionGraph::read(node).ok().and_then(|graph| {
                        // A slice inside CASE does not describe every input row.
                        graph
                            .roots
                            .iter()
                            .filter_map(|root| graph.node(root.id))
                            .find(|expr| {
                                expr.function.as_ref().is_some_and(|function| {
                                    function.set == "spql.video.frame_slice"
                                })
                            })
                            .map(|expr| graph.call_summary(expr))
                    });
                    selection.unwrap_or_else(|| "One row per selected video frame; outputs frame references, without decoding pixels".into())
                } else {
                    Self::fields(&properties["args"])
                };
                (Self::function_title(function), summary)
            }
            other => (
                other.into(),
                "Inspect this operation's native properties below".into(),
            ),
        };
        let summary_kind = if node.operator() == "Join" {
            SummaryKind::Expression
        } else {
            SummaryKind::Prose
        };
        let mut content = Self {
            title,
            summary,
            function: None,
            expression: None,
            summary_kind,
        };
        if node.operator() == "Join" && !content.summary.is_empty() {
            content.summary = format!("on {}", content.summary);
        }
        if let Ok(graph) = ExpressionGraph::read(node) {
            if node.operator() == "Filter" {
                content.summary = graph
                    .roots
                    .first()
                    .map(|root| graph.render(root.id))
                    .unwrap_or(content.summary);
                content.summary_kind = SummaryKind::Expression;
            } else if node.operator() == "Project" && graph.has_computation() {
                content.title = "Calculate columns".into();
                content.summary = graph
                    .roots
                    .iter()
                    .filter(|root| {
                        graph
                            .node(root.id)
                            .is_some_and(|expr| expr.kind != "field" && !expr.is_spql())
                    })
                    .map(|root| format!("{} = {}", root.name, graph.render(root.id)))
                    .collect::<Vec<_>>()
                    .join("; ");
                content.summary_kind = SummaryKind::Expression;
                let decision = graph.roots.iter().find(|root| {
                    graph.node(root.id).is_some_and(|expr| {
                        expr.function
                            .as_ref()
                            .is_some_and(|function| function.set == "spql.case_when")
                    })
                });
                if let Some(decision) = decision {
                    content.title = "Classify rows".into();
                    content.expression = Some(content.summary.clone());
                    let outcomes = graph.case_outcomes(decision.id);
                    content.summary = if outcomes.is_empty() {
                        graph.decision_rules()
                    } else {
                        format!("{} → {}", decision.name, outcomes.join(" · "))
                    };
                }
            }
        }
        content
    }

    /// The column names a projection keeps, in order.
    pub(super) fn projected_columns(node: &DiagnosticNode) -> String {
        node.properties()["operator"]["expressions"]
            .as_array()
            .map(|expressions| {
                expressions
                    .iter()
                    .filter_map(|expr| expr["name"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }

    /// A window that only numbers rows: its groups and the order within them.
    pub(super) fn one_row_per_group(node: &DiagnosticNode) -> Option<(String, String)> {
        let expressions = node.properties()["operator"]["expressions"].as_array()?;
        let first = expressions.first()?;
        let numbering = !expressions.is_empty()
            && expressions
                .iter()
                .all(|expr| expr["function"]["kind"] == "RowNumber");
        if !numbering {
            return None;
        }
        let groups = Self::fields(&first["partition_by"]);
        let order = Self::ordering(&first["order_by"]);
        (!groups.is_empty()).then_some((groups, order))
    }

    fn source(properties: &Value) -> Self {
        let source = &properties["source"];
        let dataset = &source["dataset"];
        let name = dataset["name"].as_str().or_else(|| dataset["id"].as_str());
        let topic = source["topic"].as_str();
        let provider = properties["provider"].as_str().unwrap_or("Unknown");
        let mut details = Vec::new();
        for (label, value) in [
            ("Dataset", &dataset["name"]),
            ("Topic", &source["topic"]),
            ("Schema variant", &source["schema_variant"]),
            ("Dataset ID", &dataset["id"]),
            ("Dataset version", &dataset["version_id"]),
            ("Data revision", &source["data_revision_id"]),
            ("Layout generation", &source["layout_generation_id"]),
        ] {
            if let Some(value) = value.as_str() {
                details.push(format!("{label}: {value}"));
            }
        }
        let title = match (name, provider) {
            (Some(name), _) => format!("Read {name}"),
            (None, "dataset_member" | "McapEpisodes") => "Read episodes".into(),
            (None, _) => "Read data".into(),
        };
        Self {
            title,
            summary: topic
                .map(str::to_owned)
                .unwrap_or_else(|| match (name, provider) {
                    (_, "McapEpisodes" | "dataset_member") => "One row per dataset episode".into(),
                    (Some(_), _) => format!("{provider} source"),
                    (None, _) => {
                        format!("{provider} source; source identity is unavailable in this plan")
                    }
                }),
            function: None,
            expression: (!details.is_empty()).then(|| details.join("\n")),
            summary_kind: SummaryKind::Prose,
        }
    }

    /// Everything worth knowing about a read, as labelled facts.
    fn source_facts(properties: &Value) -> Vec<StageFact> {
        let source = &properties["source"];
        let mut facts = Vec::new();
        if let Some(provider) = properties["provider"].as_str() {
            facts.push(StageFact::prose("Provider", provider));
        }
        for (label, value) in [
            ("Dataset", &source["dataset"]["name"]),
            ("Topic", &source["topic"]),
            ("Schema variant", &source["schema_variant"]),
            ("Dataset ID", &source["dataset"]["id"]),
            ("Dataset version", &source["dataset"]["version_id"]),
            ("Data revision", &source["data_revision_id"]),
            ("Layout generation", &source["layout_generation_id"]),
        ] {
            if let Some(value) = value.as_str() {
                facts.push(StageFact::prose(label, value));
            }
        }
        facts
    }

    pub(super) fn function_title(function: &str) -> String {
        match function {
            "spql.image.decode" | "spql.image._decode" => "Decode images".into(),
            "spql.image.to_rgb" => "Convert frames to RGB".into(),
            "spql.image.mean_luminance" => "Measure brightness".into(),
            "spql.ai.score" => "Score images".into(),
            "spql.video.from_packets" => "Build video from packets".into(),
            "spql.video.frames" => "Select video frames".into(),
            "spql.video.decode" => "Decode frames to images".into(),
            "spql.video.frame_slice" => "Select frame positions".into(),
            _ => function
                .strip_prefix("spql.")
                .unwrap_or(function)
                .replace('_', " "),
        }
    }

    fn field(field: &str) -> String {
        field
            .strip_prefix("$.")
            .or_else(|| field.strip_prefix('$'))
            .unwrap_or(field)
            .to_owned()
    }

    fn fields(fields: &Value) -> String {
        fields
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(Value::as_str)
                    .map(Self::field)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }

    fn ordering(keys: &Value) -> String {
        keys.as_array()
            .map(|keys| {
                keys.iter()
                    .map(|key| {
                        format!(
                            "{} {}",
                            Self::field(key["field"].as_str().unwrap_or_default()),
                            if key["descending"] == true {
                                "descending"
                            } else {
                                "ascending"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }

    fn limit(fetch: &Value, offset: &Value) -> String {
        let take = fetch
            .as_u64()
            .map(|count| format!("take {count} rows"))
            .unwrap_or_else(|| "take remaining rows".into());
        match offset.as_u64().unwrap_or(0) {
            0 => take,
            skip => format!("Skip {skip} rows, then {take}"),
        }
    }

    fn windows(properties: &Value) -> String {
        properties["expressions"]
            .as_array()
            .map(|expressions| {
                expressions
                    .iter()
                    .map(|expr| {
                        let groups = Self::fields(&expr["partition_by"]);
                        let order = Self::ordering(&expr["order_by"]);
                        let kind = expr["function"]["kind"]
                            .as_str()
                            .unwrap_or("Window calculation");
                        let action = match kind {
                            "RowNumber" => "Number rows starting at 1".to_owned(),
                            "Rank" => "Rank rows, leaving gaps after ties".to_owned(),
                            "DenseRank" => "Rank rows without gaps after ties".to_owned(),
                            _ => format!("{kind} → {}", expr["name"].as_str().unwrap_or("result")),
                        };
                        let mut details = vec![action];
                        if !groups.is_empty() {
                            details.push(format!("restart for each {groups}"));
                        }
                        if !order.is_empty() {
                            details.push(format!("ordered by {order}"));
                        }
                        if !matches!(kind, "RowNumber" | "Rank" | "DenseRank") {
                            details.push(format!(
                                "{} frame: {} to {}",
                                expr["frame"]["units"].as_str().unwrap_or_default(),
                                expr["frame"]["start"].as_str().unwrap_or_default(),
                                expr["frame"]["end"].as_str().unwrap_or_default()
                            ));
                        }
                        details.join("; ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "Calculate values using related rows".into())
    }
}

impl WorkflowStage {
    /// Labelled facts for the inspector, in the order the data meets them.
    pub(in super::super) fn facts(&self) -> Vec<StageFact> {
        let mut facts = Vec::new();
        if let Some(function) = &self.content.function {
            facts.push(StageFact::expression("Function", function));
            if let StageOutput::Expression { expression, dtype } = &self.output {
                facts.push(StageFact::expression("Call", expression));
                if let Some(dtype) = dtype {
                    facts.push(StageFact::expression("Produces", dtype));
                }
            } else if let Some(expression) = &self.content.expression {
                facts.push(StageFact::expression("Call", expression));
            }
            return facts;
        }
        for node in &self.operators {
            let properties = &node.properties()["operator"];
            let graph = ExpressionGraph::read(node).ok();
            match node.operator() {
                "Source" => facts.extend(StageContent::source_facts(properties)),
                "Project" => {
                    if let Some(graph) = &graph {
                        for root in &graph.roots {
                            let computed = graph
                                .node(root.id)
                                .is_some_and(|expr| expr.kind != "field" && !expr.is_spql());
                            if computed {
                                facts.push(StageFact::expression(
                                    root.name.clone(),
                                    graph.render(root.id),
                                ));
                            }
                        }
                    }
                    let kept = StageContent::projected_columns(node);
                    if !kept.is_empty() {
                        facts.push(StageFact::expression("Columns kept", kept));
                    }
                }
                "Filter" => {
                    let condition = graph
                        .as_ref()
                        .and_then(|graph| graph.roots.first().map(|root| graph.render(root.id)))
                        .or_else(|| properties["predicate"].as_str().map(str::to_owned));
                    if let Some(condition) = condition {
                        facts.push(StageFact::expression("Condition", condition));
                    }
                }
                "Join" => {
                    if let Some(kind) = properties["kind"].as_str() {
                        facts.push(StageFact::prose("Join", kind));
                    }
                    let keys = properties["on"]
                        .as_array()
                        .map(|keys| {
                            keys.iter()
                                .map(|key| {
                                    format!(
                                        "{} = {}",
                                        StageContent::field(
                                            key["left"].as_str().unwrap_or_default()
                                        ),
                                        StageContent::field(
                                            key["right"].as_str().unwrap_or_default()
                                        )
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(" and ")
                        })
                        .unwrap_or_default();
                    if !keys.is_empty() {
                        facts.push(StageFact::expression("Keys", keys));
                    }
                    if let Some(filter) = properties["filter"].as_str() {
                        facts.push(StageFact::expression("Also requires", filter));
                    }
                }
                "Window" => {
                    if let Some(expressions) = properties["expressions"].as_array() {
                        for expr in expressions {
                            let kind = expr["function"]["kind"].as_str().unwrap_or("Window");
                            let name = expr["name"].as_str().unwrap_or("result");
                            facts.push(StageFact::expression(name.to_owned(), kind.to_owned()));
                            let groups = StageContent::fields(&expr["partition_by"]);
                            if !groups.is_empty() {
                                facts.push(StageFact::expression("Groups", groups));
                            }
                            let order = StageContent::ordering(&expr["order_by"]);
                            if !order.is_empty() {
                                facts.push(StageFact::expression("Order", order));
                            }
                        }
                    }
                }
                "Aggregate" => {
                    let groups = StageContent::fields(&properties["group_by"]);
                    if !groups.is_empty() {
                        facts.push(StageFact::expression("Groups", groups));
                    }
                    let content = StageContent::relational(node);
                    if !content.summary.is_empty() {
                        facts.push(StageFact::prose("Aggregates", content.summary));
                    }
                }
                "Sort" => {
                    let order = StageContent::ordering(&properties["keys"]);
                    if !order.is_empty() {
                        facts.push(StageFact::expression("Order", order));
                    }
                }
                "Limit" => facts.push(StageFact::prose(
                    "Rows",
                    StageContent::limit(&properties["fetch"], &properties["offset"]),
                )),
                "TopK" => {
                    facts.push(StageFact::prose(
                        "Rows",
                        StageContent::limit(&properties["limit"], &properties["offset"]),
                    ));
                    let order = StageContent::ordering(&properties["keys"]);
                    if !order.is_empty() {
                        facts.push(StageFact::expression("Order", order));
                    }
                }
                "Distinct" => {
                    let by = StageContent::fields(&properties["by"]);
                    if !by.is_empty() {
                        facts.push(StageFact::expression("Distinct by", by));
                    }
                }
                "TableFunction" | "Generate" => {
                    if let Some(function) = properties["function"].as_str() {
                        facts.push(StageFact::expression("Function", function));
                    }
                    let args = StageContent::fields(&properties["args"]);
                    if !args.is_empty() {
                        facts.push(StageFact::expression("Arguments", args));
                    }
                }
                _ => {}
            }
        }
        facts
    }
}

impl ExpressionGraph {
    /// The literal outcomes of a CASE expression, in the order they are tried.
    pub(super) fn case_outcomes(&self, root: u64) -> Vec<String> {
        let mut outcomes = Vec::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let Some(expr) = self.node(id) else {
                continue;
            };
            let is_case = expr
                .function
                .as_ref()
                .is_some_and(|function| function.set == "spql.case_when");
            if !is_case {
                continue;
            }
            for branch in expr.inputs.iter().skip(1).rev() {
                match self.node(*branch) {
                    Some(leaf) if leaf.kind == "literal" => {
                        let text = Self::unquote(&leaf.display);
                        if !outcomes.contains(&text) {
                            outcomes.insert(0, text);
                        }
                    }
                    Some(_) => pending.push(*branch),
                    None => {}
                }
            }
        }
        // Branches were visited last-first so earlier outcomes lead; a nested
        // otherwise-branch keeps its own order because it was pushed after.
        outcomes
    }

    fn unquote(text: &str) -> String {
        text.trim_matches('"').trim_matches('\'').to_owned()
    }

    pub(super) fn call_summary(&self, expression: &Expression) -> String {
        let args = expression
            .inputs
            .iter()
            .map(|id| self.render(*id))
            .collect::<Vec<_>>();
        let function = expression
            .function
            .as_ref()
            .map(|function| function.set.as_str())
            .unwrap_or_default();
        match (function, args.as_slice()) {
            ("spql.video.from_packets", [packets, codec, framing]) => {
                format!("{codec} / {framing}; order {packets} by decode_index")
            }
            ("spql.video.frame_slice", [start, stop, step]) => {
                let range_end = if stop.eq_ignore_ascii_case("null") {
                    "the end of the video".into()
                } else {
                    format!("before frame {stop}")
                };
                let range_start = if start.eq_ignore_ascii_case("null") {
                    "0"
                } else {
                    start
                };
                match step.parse::<u64>().ok().filter(|step| *step > 0) {
                    Some(step) => {
                        format!("Every {step} frame positions, from {range_start} to {range_end}")
                    }
                    None => format!(
                        "Frames from {range_start} to {range_end}; step {step} (null means 1)"
                    ),
                }
            }
            ("spql.video.decode", [frame, ..]) => format!("Decode {frame} into image pixels"),
            ("spql.ai.score", [image, prompt, ..]) => {
                format!("“{}” · {image}", Self::unquote(prompt))
            }
            ("spql.image.to_rgb", [image, profile, ..]) => {
                format!("{image} as {}", Self::unquote(profile))
            }
            ("spql.image.to_rgb", [image]) => format!("{image} as RGB"),
            ("spql.image.mean_luminance", [image, ..]) => format!("Average brightness of {image}"),
            ("spql.image.decode" | "spql.image._decode", [image, ..]) => {
                format!("Read {image} into image pixels")
            }
            _ => args.join(", "),
        }
    }
}
