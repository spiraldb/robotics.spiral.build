//! A presentation graph derived only from native relational and expression edges.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use spiral_robotics_common::workflow::plan::{DiagnosticNode, DiagnosticPlan};

#[derive(Clone, PartialEq)]
pub(super) struct WorkflowStage {
    pub(super) id: String,
    pub(super) inputs: Vec<String>,
    pub(super) content: StageContent,
    pub(super) operators: Vec<DiagnosticNode>,
}
#[derive(Clone, PartialEq)]
pub(super) struct StageContent {
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) function: Option<String>,
    pub(super) expression: Option<String>,
}
impl WorkflowStage {
    fn relational(node: &DiagnosticNode) -> Self {
        Self {
            id: format!("operator-{}", node.id()),
            inputs: node
                .inputs()
                .iter()
                .map(|id| format!("operator-{id}"))
                .collect(),
            content: StageContent::relational(node),
            operators: vec![node.clone()],
        }
    }
    fn call(node: &DiagnosticNode, expression: &Expression) -> Self {
        let function = expression
            .function
            .as_ref()
            .expect("SPQL call has an identity")
            .set
            .clone();
        Self {
            id: format!("call-{}-{}", node.id(), expression.id),
            inputs: Vec::new(),
            content: StageContent {
                title: match function.as_str() {
                    "spql.image.decode" | "spql.image._decode" => "Decode images".into(),
                    "spql.image.mean_luminance" => "Measure luminance".into(),
                    "spql.ai.score" => "Score images".into(),
                    _ => function
                        .strip_prefix("spql.")
                        .unwrap_or(&function)
                        .replace('_', " "),
                },
                summary: String::new(),
                function: Some(function),
                expression: Some(expression.display.clone()),
            },
            operators: vec![node.clone()],
        }
    }
    fn mergeable(&self) -> bool {
        self.content.function.is_none()
            && self.operators.iter().all(|node| {
                matches!(
                    node.operator(),
                    "Project" | "Filter" | "Aggregate" | "Sort" | "Limit" | "TopK" | "Distinct"
                )
            })
    }
    fn refresh_summary(&mut self) {
        let has = |operator| {
            self.operators
                .iter()
                .any(|node| node.operator() == operator)
        };
        let classify = self
            .operators
            .iter()
            .any(|node| StageContent::relational(node).title == "Classify rows");
        self.content.title = match (
            has("Filter"),
            has("Aggregate"),
            classify,
            has("Sort") || has("TopK"),
        ) {
            (true, true, _, _) => "Filter & summarize",
            (_, true, _, _) => "Summarize rows",
            (_, _, true, true) => "Classify & order",
            (_, _, true, _) => "Classify rows",
            (true, _, _, _) => "Filter & prepare",
            (_, _, _, true) => "Prepare results",
            _ => "Prepare columns",
        }
        .into();
        self.content.summary = self
            .operators
            .iter()
            .filter(|node| {
                node.operator() != "Project"
                    || ExpressionGraph::read(node).is_ok_and(|graph| graph.has_computation())
            })
            .map(|node| StageContent::relational(node).summary)
            .filter(|summary| !summary.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
    }
}
impl StageContent {
    fn relational(node: &DiagnosticNode) -> Self {
        let properties = &node.properties()["operator"];
        let (title, summary) = match node.operator() {
            "Source" => (
                "Read data",
                properties["provider"]
                    .as_str()
                    .unwrap_or("Source relation")
                    .to_owned(),
            ),
            "Values" => (
                "Input rows",
                properties
                    .get("rows")
                    .map(|rows| format!("{rows} rows"))
                    .unwrap_or_default(),
            ),
            "Project" => (
                "Select & compute",
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
                "Filter rows",
                properties["predicate"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ),
            "Aggregate" => (
                "Summarize",
                properties["aggregates"]
                    .as_array()
                    .map(|aggregates| {
                        aggregates
                            .iter()
                            .filter_map(|agg| agg["name"].as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ),
            "Sort" | "TopK" => (
                "Order rows",
                properties["keys"]
                    .as_array()
                    .map(|keys| {
                        keys.iter()
                            .map(|key| {
                                format!(
                                    "{} {}",
                                    key["field"].as_str().unwrap_or_default(),
                                    if key["descending"].as_bool() == Some(true) {
                                        "descending"
                                    } else {
                                        "ascending"
                                    }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ),
            "Limit" => (
                "Limit rows",
                format!(
                    "{} rows; skip {}",
                    properties["fetch"], properties["offset"]
                ),
            ),
            "Distinct" => ("Remove duplicates", String::new()),
            "Join" | "AsOfJoin" => (
                "Join data",
                properties["kind"].as_str().unwrap_or_default().to_owned(),
            ),
            "TableFunction" | "Generate" => (
                "Read / expand",
                properties["function"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ),
            other => (other, String::new()),
        };
        let mut content = Self {
            title: title.into(),
            summary,
            function: None,
            expression: None,
        };
        if node.operator() == "Source" {
            let fields = node
                .output()
                .pointer("/schema/fields")
                .and_then(serde_json::Value::as_array);
            if let Some(fields) = fields {
                content.summary = fields
                    .iter()
                    .filter_map(|field| field["name"].as_str())
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(", ");
                if fields.len() > 3 {
                    content
                        .summary
                        .push_str(&format!(" +{} columns", fields.len() - 3));
                }
            }
        }
        if let Ok(graph) = ExpressionGraph::read(node) {
            if node.operator() == "Filter" {
                content.summary = graph
                    .roots
                    .first()
                    .map(|root| graph.render(root.id))
                    .unwrap_or(content.summary);
            } else if node.operator() == "Project" && graph.has_computation() {
                let computed = graph.roots.iter().filter(|root| {
                    graph
                        .node(root.id)
                        .is_some_and(|expr| expr.kind != "field" && !expr.is_spql())
                });
                content.summary = computed
                    .map(|root| format!("{} = {}", root.name, graph.render(root.id)))
                    .collect::<Vec<_>>()
                    .join("; ");
                if graph.nodes.iter().any(|expr| {
                    expr.function
                        .as_ref()
                        .is_some_and(|function| function.set == "spql.case_when")
                }) {
                    content.title = "Classify rows".into();
                    content.expression = Some(content.summary.clone());
                    content.summary = graph.decision_rules();
                }
            }
        }
        content
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct WorkflowGraph {
    pub(super) stages: Vec<WorkflowStage>,
    pub(super) root: String,
}
impl WorkflowGraph {
    pub(super) fn from_plan(plan: &DiagnosticPlan) -> Result<Self, String> {
        let mut stages = Vec::new();
        let mut outputs = BTreeMap::new();
        for node in plan.nodes() {
            let graph = ExpressionGraph::read(node)?;
            let calls = graph
                .nodes
                .iter()
                .filter(|expression| expression.is_spql())
                .collect::<Vec<_>>();
            let mut relational = WorkflowStage::relational(node);
            let mut consumed_calls = BTreeSet::new();
            for expression in &calls {
                let mut stage = WorkflowStage::call(node, expression);
                let dependencies = graph.calls_below(&expression.inputs);
                consumed_calls.extend(dependencies.iter().copied());
                stage.inputs = if dependencies.is_empty() {
                    relational.inputs.clone()
                } else {
                    dependencies
                        .iter()
                        .map(|id| format!("call-{}-{id}", node.id()))
                        .collect()
                };
                let names = graph
                    .roots
                    .iter()
                    .filter(|root| root.id == expression.id)
                    .map(|root| root.name.as_str())
                    .collect::<Vec<_>>();
                stage.content.summary = if names.is_empty() {
                    expression
                        .inputs
                        .iter()
                        .filter_map(|id| graph.node(*id))
                        .map(|arg| arg.display.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    format!("Produces {}", names.join(", "))
                };
                if let Some(condition) = graph.condition_for(expression.id) {
                    stage.content.summary = format!("Used when {condition}");
                }
                stages.push(stage);
            }
            let terminals = calls
                .iter()
                .filter(|call| !consumed_calls.contains(&call.id))
                .map(|call| format!("call-{}-{}", node.id(), call.id))
                .collect::<Vec<_>>();
            // A lone scalar result plus passthrough columns needs no separate projection box.
            let passthrough = node.operator() == "Project"
                && terminals.len() == 1
                && graph.roots.iter().all(|root| {
                    graph
                        .node(root.id)
                        .is_some_and(|expr| expr.kind == "field" || expr.is_spql())
                });
            if passthrough {
                outputs.insert(relational.id, terminals[0].clone());
            } else {
                if !terminals.is_empty() {
                    relational.inputs = terminals;
                }
                outputs.insert(relational.id.clone(), relational.id.clone());
                stages.push(relational);
            }
        }
        for stage in &mut stages {
            for input in &mut stage.inputs {
                if let Some(output) = outputs.get(input) {
                    *input = output.clone();
                }
            }
        }
        let mut graph = Self {
            stages,
            root: outputs[&format!("operator-{}", plan.root())].clone(),
        };
        graph.collapse_sql();
        Ok(graph)
    }
    fn collapse_sql(&mut self) {
        loop {
            let mut consumers = BTreeMap::<String, usize>::new();
            for stage in &self.stages {
                for input in &stage.inputs {
                    *consumers.entry(input.clone()).or_default() += 1;
                }
            }
            let pair = self.stages.iter().enumerate().find_map(|(down, stage)| {
                if !stage.mergeable() || stage.inputs.len() != 1 {
                    return None;
                }
                self.stages
                    .iter()
                    .position(|up| {
                        up.id == stage.inputs[0]
                            && up.id != stage.id
                            && up.id != self.root
                            && up.mergeable()
                            && consumers.get(&up.id) == Some(&1)
                    })
                    .map(|up| (up, down))
            });
            let Some((up, down)) = pair else {
                break;
            };
            let upstream = self.stages[up].clone();
            let downstream = &mut self.stages[down];
            downstream.inputs = upstream.inputs;
            downstream.operators.splice(0..0, upstream.operators);
            downstream.refresh_summary();
            self.stages.remove(up);
        }
    }
}

#[derive(Deserialize)]
struct ExpressionGraph {
    schema_version: u32,
    complete: bool,
    roots: Vec<ExpressionRoot>,
    nodes: Vec<Expression>,
}
#[derive(Deserialize)]
struct ExpressionRoot {
    id: u64,
    name: String,
}
#[derive(Deserialize)]
struct Expression {
    id: u64,
    kind: String,
    inputs: Vec<u64>,
    function: Option<FunctionIdentity>,
    display: String,
}
#[derive(Deserialize)]
struct FunctionIdentity {
    set: String,
}
impl Expression {
    fn is_spql(&self) -> bool {
        self.function.as_ref().is_some_and(|function| {
            // SQL arithmetic, predicates, and standard scalar functions stay with their SQL stage.
            let Some(name) = function.set.strip_prefix("spql.") else {
                return false;
            };
            match name.split_once('.') {
                Some((namespace, _)) => !matches!(namespace, "math" | "str" | "dt" | "agg"),
                None => !matches!(
                    name,
                    "eq" | "neq"
                        | "lt"
                        | "lte"
                        | "gt"
                        | "gte"
                        | "and"
                        | "or"
                        | "not"
                        | "add"
                        | "sub"
                        | "mul"
                        | "div"
                        | "mod"
                        | "neg"
                        | "case_when"
                        | "is_null"
                        | "is_not_null"
                        | "coalesce"
                        | "like"
                        | "ilike"
                        | "in"
                        | "between"
                ),
            }
        })
    }
}
impl ExpressionGraph {
    fn read(node: &DiagnosticNode) -> Result<Self, String> {
        let metadata = node.properties().get("expressions").ok_or("The workflow graph needs Spiral's native expression metadata. Update the local driver and plan the SQL again.")?;
        let graph: Self = serde_json::from_value(metadata.clone())
            .map_err(|_| "Spiral returned invalid expression metadata.")?;
        let ids = graph
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<BTreeSet<_>>();
        if graph.schema_version != 1
            || ids.len() != graph.nodes.len()
            || graph.roots.iter().any(|root| !ids.contains(&root.id))
            || graph.nodes.iter().any(|node| {
                node.inputs
                    .iter()
                    .any(|id| *id <= node.id || !ids.contains(id))
            })
        {
            return Err("Spiral returned unsupported or invalid expression dependencies.".into());
        }
        if !graph.complete {
            return Err("Spiral did not report complete dependencies for this SQL. The workflow graph cannot display its opaque or subquery expressions.".into());
        }
        Ok(graph)
    }
    fn condition_for(&self, expression: u64) -> Option<String> {
        let mut pending = vec![expression];
        let mut conditions = Vec::new();
        while let Some(child) = pending.pop() {
            for parent in &self.nodes {
                if let Some(argument) = parent.inputs.iter().position(|id| *id == child) {
                    if parent
                        .function
                        .as_ref()
                        .is_some_and(|function| function.set == "spql.case_when")
                        && argument > 0
                    {
                        let condition = self.render(parent.inputs[0]);
                        conditions.push(if argument == 1 {
                            condition
                        } else {
                            format!("({condition}) is not true")
                        });
                    }
                    pending.push(parent.id);
                }
            }
        }
        (!conditions.is_empty()).then(|| conditions.join(" AND "))
    }
    fn decision_rules(&self) -> String {
        let mut rules = Vec::new();
        for expr in &self.nodes {
            if expr.function.as_ref().is_some_and(|function| {
                matches!(
                    function.set.as_str(),
                    "spql.lt" | "spql.lte" | "spql.gt" | "spql.gte" | "spql.eq" | "spql.neq"
                )
            }) {
                let rule = self.render(expr.id);
                if !rules.contains(&rule) {
                    rules.push(rule);
                }
            }
        }
        let more = rules.len().saturating_sub(2);
        let mut summary = rules.into_iter().take(2).collect::<Vec<_>>().join("; ");
        if more > 0 {
            summary.push_str(&format!("; +{more} rules"));
        }
        if summary.is_empty() {
            "Conditional output values; inspect the stage for cases".into()
        } else {
            summary
        }
    }
    fn has_computation(&self) -> bool {
        self.roots.iter().any(|root| {
            self.node(root.id)
                .is_some_and(|expr| expr.kind != "field" && !expr.is_spql())
        })
    }
    fn render(&self, root: u64) -> String {
        let mut rendered = BTreeMap::<u64, String>::new();
        let ordered = self
            .nodes
            .iter()
            .map(|node| (node.id, node))
            .collect::<BTreeMap<_, _>>();
        for (id, expr) in ordered.iter().rev() {
            let arguments = expr
                .inputs
                .iter()
                .map(|id| rendered.get(id).cloned().unwrap_or_default())
                .collect::<Vec<_>>();
            let function = expr
                .function
                .as_ref()
                .map(|function| function.set.as_str())
                .unwrap_or_default();
            let binary = match function {
                "spql.lt" => "<",
                "spql.lte" => "≤",
                "spql.gt" => ">",
                "spql.gte" => "≥",
                "spql.eq" => "=",
                "spql.neq" => "≠",
                "spql.and" => "AND",
                "spql.or" => "OR",
                _ => "",
            };
            let value = match arguments.as_slice() {
                [left, right] if !binary.is_empty() => format!("({left} {binary} {right})"),
                [condition, yes, no] if function == "spql.case_when" => {
                    format!("when {condition}: {yes}; otherwise {no}")
                }
                [input] if function == "spql.is_null" => format!("{input} is null"),
                [input] if function == "spql.not" => format!("NOT ({input})"),
                _ if expr.kind == "field" => expr
                    .display
                    .strip_prefix("$.")
                    .unwrap_or(&expr.display)
                    .to_owned(),
                _ => expr.display.clone(),
            };
            rendered.insert(*id, value);
        }
        rendered.remove(&root).unwrap_or_default()
    }
    fn node(&self, id: u64) -> Option<&Expression> {
        self.nodes.iter().find(|node| node.id == id)
    }
    fn calls_below(&self, inputs: &[u64]) -> BTreeSet<u64> {
        let mut pending = inputs.to_vec();
        let mut calls = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if let Some(expr) = self.node(id) {
                if expr.is_spql() {
                    calls.insert(id);
                } else {
                    pending.extend(&expr.inputs);
                }
            }
        }
        calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    struct Fixture;
    impl Fixture {
        fn node(id: u64, operator: &str, inputs: &[u64]) -> Value {
            json!({"id":id,"operator":operator,"inputs":inputs,"output":{},
                "properties":{"operator":{},"expressions":{"schema_version":1,"complete":true,"roots":[],"nodes":[]}}})
        }
        fn call(id: u64, function: &str, inputs: &[u64]) -> Value {
            json!({"id":id,"kind":"call","function":{"set":function,"overload":"default"},"inputs":inputs,"display":function})
        }
        fn expressions(node: &mut Value, nodes: Vec<Value>) {
            node["properties"]["expressions"]["roots"] = json!([{"name":"result","id":0}]);
            node["properties"]["expressions"]["nodes"] = json!(nodes);
        }
        fn plan(nodes: Vec<Value>) -> DiagnosticPlan {
            serde_json::from_value(json!({"schema_version":1,"root":0,"nodes":nodes})).unwrap()
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn nested_operations_are_ordered_and_surrounding_sql_is_collapsed() {
        let mut project = Fixture::node(3, "Project", &[4]);
        Fixture::expressions(
            &mut project,
            vec![
                Fixture::call(0, "spql.image.mean_luminance", &[1]),
                Fixture::call(1, "spql.image.decode", &[2]),
                json!({"id":2,"kind":"field","inputs":[],"display":"$.image"}),
            ],
        );
        let plan = Fixture::plan(vec![
            Fixture::node(0, "Project", &[1]),
            Fixture::node(1, "Sort", &[2]),
            Fixture::node(2, "Filter", &[3]),
            project,
            Fixture::node(4, "Source", &[]),
        ]);
        let original = plan.clone();
        let graph = WorkflowGraph::from_plan(&plan).unwrap();
        assert_eq!(graph.stages.len(), 4);
        let decode = graph
            .stages
            .iter()
            .find(|stage| stage.id == "call-3-1")
            .unwrap();
        let measure = graph
            .stages
            .iter()
            .find(|stage| stage.id == "call-3-0")
            .unwrap();
        assert_eq!(decode.inputs, ["operator-4"]);
        assert_eq!(measure.inputs, ["call-3-1"]);
        let output = graph
            .stages
            .iter()
            .find(|stage| stage.id == graph.root)
            .unwrap();
        assert_eq!(output.inputs, ["call-3-0"]);
        assert_eq!(
            output
                .operators
                .iter()
                .map(DiagnosticNode::id)
                .collect::<Vec<_>>(),
            [2, 1, 0]
        );
        assert_eq!(plan, original);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn independent_calls_and_repeated_shared_inputs_do_not_become_a_sequence() {
        let mut project = Fixture::node(2, "Project", &[3]);
        Fixture::expressions(
            &mut project,
            vec![
                Fixture::call(0, "spql.image.width", &[2]),
                Fixture::call(1, "spql.custom_measure", &[3]),
                json!({"id":2,"kind":"field","inputs":[],"display":"$.image"}),
                json!({"id":3,"kind":"field","inputs":[],"display":"$.image"}),
            ],
        );
        project["properties"]["expressions"]["roots"] =
            json!([{"id":0,"name":"width"},{"id":1,"name":"height"}]);
        let plan = Fixture::plan(vec![
            Fixture::node(0, "Join", &[1, 1]),
            Fixture::node(1, "Filter", &[2]),
            project,
            Fixture::node(3, "Source", &[]),
        ]);
        let graph = WorkflowGraph::from_plan(&plan).unwrap();
        let calls = graph
            .stages
            .iter()
            .filter(|stage| stage.content.function.is_some())
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|stage| stage.inputs == ["operator-3"]));
        let joined = graph
            .stages
            .iter()
            .find(|stage| stage.id == graph.root)
            .unwrap();
        assert_eq!(joined.inputs, ["operator-1", "operator-1"]);
        let shared = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-1")
            .unwrap();
        assert_eq!(shared.inputs, ["call-2-0", "call-2-1"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn predicates_are_readable_and_literal_function_names_are_not_operations() {
        let mut filter = Fixture::node(0, "Filter", &[1]);
        Fixture::expressions(
            &mut filter,
            vec![
                Fixture::call(0, "spql.lt", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"luminance"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"0.03"}),
                json!({"id":3,"kind":"literal","inputs":[],"display":"spql.image.decode(x)"}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            filter,
            Fixture::node(1, "Source", &[]),
        ]))
        .unwrap();
        assert_eq!(graph.stages.len(), 2);
        assert_eq!(graph.stages[0].content.summary, "(luminance < 0.03)");
        assert!(
            graph
                .stages
                .iter()
                .all(|stage| stage.content.function.is_none())
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn conditional_spql_calls_retain_their_guard_in_the_stage_summary() {
        let mut project = Fixture::node(0, "Project", &[1]);
        Fixture::expressions(
            &mut project,
            vec![
                Fixture::call(0, "spql.case_when", &[1, 2, 3]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.needs_review"}),
                Fixture::call(2, "spql.ai.score", &[4]),
                json!({"id":3,"kind":"literal","inputs":[],"display":"null"}),
                json!({"id":4,"kind":"field","inputs":[],"display":"$.image"}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            project,
            Fixture::node(1, "Source", &[]),
        ]))
        .unwrap();
        let call = graph
            .stages
            .iter()
            .find(|stage| stage.content.function.is_some())
            .unwrap();
        assert_eq!(call.content.summary, "Used when needs_review");
        assert_ne!(graph.root, call.id);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn absent_metadata_and_invalid_expression_edges_are_explicit_errors() {
        let mut node = Fixture::node(0, "Project", &[]);
        node["properties"]
            .as_object_mut()
            .unwrap()
            .remove("expressions");
        assert!(WorkflowGraph::from_plan(&Fixture::plan(vec![node])).is_err());
        let mut node = Fixture::node(0, "Project", &[]);
        Fixture::expressions(&mut node, vec![Fixture::call(0, "spql.image.decode", &[0])]);
        assert!(WorkflowGraph::from_plan(&Fixture::plan(vec![node])).is_err());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn incomplete_dependencies_are_rejected_for_every_operator() {
        for operator in ["Project", "Join", "Window", "Source", "Extension"] {
            let mut node = Fixture::node(0, operator, &[]);
            node["properties"]["expressions"]["complete"] = json!(false);
            assert!(WorkflowGraph::from_plan(&Fixture::plan(vec![node])).is_err());
        }
    }
}
