//! A presentation graph derived only from native relational and expression edges.
mod descriptions;

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use spiral_robotics_common::workflow::plan::{DiagnosticNode, DiagnosticPlan};

#[derive(Clone, PartialEq)]
pub(super) struct WorkflowStage {
    pub(super) id: String,
    pub(super) inputs: Vec<String>,
    pub(super) content: StageContent,
    pub(super) operators: Vec<DiagnosticNode>,
    output: StageOutput,
}
/// Relational edges carry rows; expression edges carry one native typed value.
#[derive(Clone, PartialEq)]
pub(super) enum StageOutput {
    Relation(serde_json::Value),
    Expression {
        expression: String,
        dtype: Option<String>,
    },
}

#[derive(Clone, PartialEq)]
pub(super) struct StageContent {
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) function: Option<String>,
    pub(super) expression: Option<String>,
    /// Whether the summary is a rendered expression (fields, literals,
    /// operators) or a sentence. Expressions are painted token by token.
    pub(super) summary_kind: SummaryKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SummaryKind {
    Prose,
    Expression,
}

/// One labelled thing worth knowing about a stage: the dataset it reads, the
/// keys it joins on, the condition it keeps, the columns it computes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StageFact {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) kind: SummaryKind,
}

impl StageFact {
    fn prose(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            kind: SummaryKind::Prose,
        }
    }

    fn expression(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            kind: SummaryKind::Expression,
        }
    }
}

/// What a stage does to the data. It drives the stage glyph and identity tint
/// only; the native operators remain the truth about what executes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StageKind {
    /// Rows read from a Spiral relation or literal values.
    Source,
    /// Native video and image functions that produce or transform pixels.
    Media,
    /// Native AI inference over images or text.
    Model,
    /// Ordinary SQL shaping: filters, projections, grouping, ordering.
    Shape,
    /// Two row populations combined.
    Join,
    /// The relation the workflow returns.
    Output,
}

impl StageKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Media => "Media",
            Self::Model => "Model",
            Self::Shape => "SQL",
            Self::Join => "Join",
            Self::Output => "Output",
        }
    }

    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Media => "media",
            Self::Model => "model",
            Self::Shape => "shape",
            Self::Join => "join",
            Self::Output => "output",
        }
    }
}

impl WorkflowStage {
    pub(super) fn kind(&self, root: &str) -> StageKind {
        if let Some(function) = &self.content.function {
            return if function.starts_with("spql.ai.") {
                StageKind::Model
            } else {
                StageKind::Media
            };
        }
        if self.reads_source() {
            return StageKind::Source;
        }
        if self.combines_inputs() {
            return StageKind::Join;
        }
        if self.id == root {
            return StageKind::Output;
        }
        StageKind::Shape
    }

    /// A stage that begins by reading rows, whatever shaping was folded in.
    pub(super) fn reads_source(&self) -> bool {
        self.operators
            .first()
            .is_some_and(|node| matches!(node.operator(), "Source" | "Values"))
    }

    pub(super) fn is_join(&self) -> bool {
        self.operators.iter().any(|node| node.operator() == "Join")
    }

    /// A set operation stacks or intersects two row populations. Its inputs
    /// are peers, so unlike a join they carry no left/right labels.
    pub(super) fn is_set_operation(&self) -> bool {
        self.operators
            .iter()
            .any(|node| matches!(node.operator(), "SetOp" | "Union" | "Intersect" | "Except"))
    }

    /// Whether this stage brings two row populations together.
    fn combines_inputs(&self) -> bool {
        self.is_join() || self.is_set_operation()
    }

    /// The footer line under a stage: the native function it calls, the
    /// provider it reads from, or the SQL operations folded into it, named in
    /// execution order ("filter · number · select").
    pub(super) fn footer(&self, root: &str) -> String {
        if let Some(function) = &self.content.function {
            return function.clone();
        }
        if self.kind(root) == StageKind::Source
            && let Some(provider) = self
                .operators
                .first()
                .and_then(|node| node.properties()["operator"]["provider"].as_str())
        {
            return provider.to_owned();
        }
        let mut words = Vec::<&str>::new();
        for node in &self.operators {
            let word = match node.operator() {
                "Filter" => "filter",
                "Project" => "select",
                "Sort" => "sort",
                "Limit" | "TopK" => "limit",
                "Aggregate" => "group",
                "Window" => "number",
                "Distinct" => "distinct",
                "Join" => "join",
                "SetOp" | "Union" => "union",
                "Intersect" => "intersect",
                "Except" => "except",
                "Values" => "values",
                "TableFunction" | "Generate" => "expand",
                other => other,
            };
            if !words.contains(&word) {
                words.push(word);
            }
        }
        words.join(" · ")
    }

    /// Column names this stage hands on, when it hands on rows.
    pub(super) fn output_columns(&self) -> Vec<String> {
        match &self.output {
            StageOutput::Relation(output) => output
                .pointer("/schema/fields")
                .and_then(serde_json::Value::as_array)
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| field["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            StageOutput::Expression { .. } => Vec::new(),
        }
    }
    fn relational(node: &DiagnosticNode) -> Self {
        Self {
            id: format!("operator-{}", node.id()),
            inputs: node
                .inputs()
                .iter()
                .map(|id| format!("operator-{id}"))
                .collect(),
            content: StageContent::relational(node),
            output: StageOutput::Relation(node.output().clone()),
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
            output: StageOutput::Expression {
                expression: expression.display.clone(),
                dtype: expression.dtype.clone(),
            },
            content: StageContent {
                title: StageContent::function_title(&function),
                summary: String::new(),
                function: Some(function),
                expression: Some(expression.display.clone()),
                summary_kind: SummaryKind::Prose,
            },
            operators: vec![node.clone()],
        }
    }
    pub(super) fn output(&self) -> &StageOutput {
        &self.output
    }

    fn mergeable(&self) -> bool {
        self.content.function.is_none()
            && self.operators.iter().all(|node| {
                matches!(
                    node.operator(),
                    "Project"
                        | "Filter"
                        | "Aggregate"
                        | "Sort"
                        | "Limit"
                        | "TopK"
                        | "Distinct"
                        | "Window"
                )
            })
    }
    fn refresh_summary(&mut self) {
        // A native call keeps its own name and summary whatever shaping was
        // folded in after it; the inspector lists the folded steps.
        if self.content.function.is_some() {
            return;
        }
        let has = |operator| {
            self.operators
                .iter()
                .any(|node| node.operator() == operator)
        };
        if self.reads_source() {
            self.content = StageContent::relational(&self.operators[0]);
            let kept = self
                .operators
                .iter()
                .rev()
                .find(|node| node.operator() == "Project")
                .map(StageContent::projected_columns)
                .filter(|columns| !columns.is_empty());
            if let Some(columns) = kept {
                self.content.summary = format!("{} → {columns}", self.content.summary);
            }
            return;
        }
        if let Some(combining) = self.operators.iter().find(|node| {
            matches!(
                node.operator(),
                "Join" | "SetOp" | "Union" | "Intersect" | "Except"
            )
        }) {
            self.content = StageContent::relational(combining);
            return;
        }
        // Numbering rows inside groups and then keeping row 1 is one idea:
        // pick one row per group. The filter has to come after the window;
        // a filter before it is an unrelated condition.
        let numbering = self
            .operators
            .iter()
            .position(|node| node.operator() == "Window")
            .and_then(|index| {
                let keeps_first = self.operators[index + 1..].iter().any(|node| {
                    node.operator() == "Filter"
                        && StageContent::relational(node).summary.contains("= 1)")
                });
                keeps_first
                    .then(|| StageContent::one_row_per_group(&self.operators[index]))
                    .flatten()
            });
        if let Some((groups, order)) = numbering {
            self.content = StageContent {
                title: format!("One row per {groups}"),
                summary: format!("First by {order}"),
                function: None,
                expression: None,
                summary_kind: SummaryKind::Prose,
            };
            return;
        }
        let classify = self
            .operators
            .iter()
            .any(|node| StageContent::relational(node).title == "Classify rows");
        // Lead with the operation that changes the row population, then explain
        // the supporting calculations in native execution order in the inspector.
        let priority = [
            "Aggregate",
            "Limit",
            "TopK",
            "Filter",
            "Window",
            "Sort",
            "Distinct",
            "Project",
        ];
        let lead = priority
            .iter()
            .find_map(|kind| self.operators.iter().find(|node| node.operator() == *kind));
        if let Some(lead) = lead {
            self.content = StageContent::relational(lead);
            // Every condition the stage applies, not only the first one.
            if lead.operator() == "Filter" {
                let conditions = self
                    .operators
                    .iter()
                    .filter(|node| node.operator() == "Filter")
                    .map(|node| StageContent::relational(node).summary)
                    .filter(|condition| !condition.is_empty())
                    .collect::<Vec<_>>();
                if conditions.len() > 1 {
                    self.content.summary = conditions.join(" AND ");
                    self.content.summary_kind = SummaryKind::Expression;
                }
            }
        }
        if classify && !has("Aggregate") && !has("Limit") && !has("TopK") {
            self.content.title = if has("Filter") {
                "Classify and keep rows".into()
            } else {
                "Classify rows".into()
            };
        }
    }

    /// Column selection and ordering directly after a stage are how that
    /// stage hands its rows on, not separate ideas; they fold into it. A
    /// projection that computes something is a step of its own and stays out.
    fn folds_into_upstream(&self) -> bool {
        self.content.function.is_none()
            && self.inputs.len() == 1
            && self.operators.iter().all(|node| match node.operator() {
                "Sort" => true,
                "Project" => {
                    !ExpressionGraph::read(node).is_ok_and(|graph| graph.has_computation())
                }
                _ => false,
            })
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
                stage.content.summary = graph.call_summary(expression);
                if let Some(condition) = graph.condition_for(expression.id) {
                    stage.content.summary =
                        format!("{}; only when {condition}", stage.content.summary);
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
                // This call also represents the completed projection, including
                // passthrough columns, so its outgoing edge carries the row schema.
                stages
                    .iter_mut()
                    .find(|stage| stage.id == terminals[0])
                    .expect("terminal call was added above")
                    .output = relational.output;
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
        graph.fold_shaping();
        graph.collapse_sql();
        Ok(graph)
    }
    fn consumers(&self) -> BTreeMap<String, usize> {
        let mut consumers = BTreeMap::<String, usize>::new();
        for stage in &self.stages {
            for input in &stage.inputs {
                *consumers.entry(input.clone()).or_default() += 1;
            }
        }
        consumers
    }
    /// Pure column selection and ordering fold into whichever stage feeds
    /// them, as long as that stage feeds nothing else: a read keeps its
    /// projection, a native call keeps the rename after it, a join keeps the
    /// columns it hands on.
    fn fold_shaping(&mut self) {
        loop {
            let consumers = self.consumers();
            let pair = self.stages.iter().enumerate().find_map(|(down, stage)| {
                if !stage.folds_into_upstream() {
                    return None;
                }
                self.stages
                    .iter()
                    .position(|up| {
                        up.id == stage.inputs[0]
                            && up.id != stage.id
                            && !up.mergeable()
                            && consumers.get(&up.id) == Some(&1)
                    })
                    .map(|up| (up, down))
            });
            let Some((up, down)) = pair else {
                break;
            };
            let downstream = self.stages.remove(down);
            let up = if down < up { up - 1 } else { up };
            let upstream_id = self.stages[up].id.clone();
            for stage in &mut self.stages {
                for input in &mut stage.inputs {
                    if *input == downstream.id {
                        *input = upstream_id.clone();
                    }
                }
            }
            if self.root == downstream.id {
                self.root = upstream_id;
            }
            let upstream = &mut self.stages[up];
            upstream.operators.extend(downstream.operators);
            upstream.output = downstream.output;
            upstream.refresh_summary();
        }
    }
    /// A linear run of routine SQL between two meaningful stages is one idea:
    /// the rows are shaped, then handed on. The inspector keeps every step.
    fn collapse_sql(&mut self) {
        loop {
            let consumers = self.consumers();
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
    dtype: Option<String>,
}
#[derive(Deserialize)]
struct FunctionIdentity {
    set: String,
}
impl Expression {
    fn is_spql(&self) -> bool {
        self.function.as_ref().is_some_and(|function| {
            // SQL arithmetic, predicates, and standard scalar functions stay with their SQL stage.
            if function.set == "spql.video.frame_slice" {
                return false;
            }
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
                "spql.add" => "+",
                "spql.sub" => "−",
                "spql.mul" => "×",
                "spql.div" => "/",
                _ => "",
            };
            let value = match arguments.as_slice() {
                [left, right] if !binary.is_empty() => format!("({left} {binary} {right})"),
                [condition, yes, no] if function == "spql.case_when" => {
                    format!("when {condition}: {yes}; otherwise {no}")
                }
                [input] if function == "spql.is_null" => format!("{input} is null"),
                [input] if function == "spql.not" => format!("NOT ({input})"),
                // A cast changes a value's type, not its meaning; the stored
                // type stays in the native operator JSON.
                [input] if expr.kind == "cast" => input.clone(),
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
    fn collapsed_stage_exposes_its_final_projection_schema() {
        let mut filter = Fixture::node(2, "Filter", &[3]);
        filter["output"] =
            json!({"schema":{"fields":[{"name":"raw", "dtype":"i64", "nullable":false}]}});
        let mut project = Fixture::node(1, "Project", &[2]);
        project["output"] =
            json!({"schema":{"fields":[{"name":"renamed", "dtype":"i64?", "nullable":true}]}});
        let expected = project["output"].clone();
        let plan = Fixture::plan(vec![
            Fixture::node(0, "Sink", &[1]),
            project,
            filter,
            Fixture::node(3, "Source", &[]),
        ]);
        let graph = WorkflowGraph::from_plan(&plan).unwrap();
        let stage = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-1")
            .unwrap();
        assert_eq!(stage.operators.len(), 2);
        assert!(matches!(stage.output(), StageOutput::Relation(output) if output == &expected));
        assert_eq!(graph.stages[0].inputs, ["operator-1"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn nested_calls_expose_value_types_until_the_projection_is_complete() {
        let mut project = Fixture::node(1, "Project", &[2]);
        project["output"] = json!({"schema":{"fields":[
            {"name":"episode_id", "dtype":"i64", "nullable":false},
            {"name":"image", "dtype":"spql.image.Image", "nullable":false}
        ]}});
        let expected = project["output"].clone();
        let mut outer = Fixture::call(0, "spql.image.resize", &[1]);
        outer["dtype"] = json!("spql.image.Image");
        let mut inner = Fixture::call(1, "spql.video.decode", &[]);
        inner["dtype"] = json!("spql.image.Image?");
        Fixture::expressions(
            &mut project,
            vec![
                outer,
                inner,
                json!({"id":2,"kind":"field","inputs":[],"display":"$.episode_id","dtype":"i64"}),
            ],
        );
        project["properties"]["expressions"]["roots"] = json!([
            {"name":"image","id":0}, {"name":"episode_id","id":2}
        ]);
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Sink", &[1]),
            project,
            Fixture::node(2, "Source", &[]),
        ]))
        .unwrap();
        let inner = graph
            .stages
            .iter()
            .find(|stage| stage.id == "call-1-1")
            .unwrap();
        assert!(
            matches!(inner.output(), StageOutput::Expression { dtype: Some(dtype), .. } if dtype == "spql.image.Image?")
        );
        let outer = graph
            .stages
            .iter()
            .find(|stage| stage.id == "call-1-0")
            .unwrap();
        assert_eq!(outer.inputs, ["call-1-1"]);
        assert!(matches!(outer.output(), StageOutput::Relation(output) if output == &expected));
        assert_eq!(graph.stages[0].inputs, ["call-1-0"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn packet_workflow_explains_sources_numbering_and_frame_selection() {
        let mut source = Fixture::node(7, "Source", &[]);
        source["properties"]["operator"] = json!({"provider":"McapTopic", "source":{"dataset":{"name":"dataset_default", "id":"dataset-id", "version_id":"version-id"}, "topic":"/camera/follower.camera.gripper/video", "schema_variant":"variant"}});
        let mut window = Fixture::node(6, "Window", &[7]);
        window["properties"]["operator"] = json!({"expressions":[{
            "name":"__window_expr_0", "function":{"kind":"RowNumber"},
            "partition_by":["$.id"], "order_by":[
                {"field":"$chunk_start_offset","descending":false},
                {"field":"$sample_ordinal","descending":false}
            ]
        }]});
        let mut aggregate = Fixture::node(4, "Aggregate", &[5]);
        aggregate["properties"]["operator"] = json!({"group_by":["episode_id"],
            "aggregates":[{"function":"spiral.list.collect","input":"$.packet","name":"packets","distinct":false}]});
        let mut video = Fixture::node(3, "Project", &[4]);
        Fixture::expressions(
            &mut video,
            vec![
                Fixture::call(0, "spql.video.from_packets", &[1, 2, 3]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.packets"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"\"h264\""}),
                json!({"id":3,"kind":"literal","inputs":[],"display":"\"annex_b\""}),
            ],
        );
        let mut limit = Fixture::node(2, "Limit", &[3]);
        limit["properties"]["operator"] = json!({"fetch":2,"offset":10});
        let mut frames = Fixture::node(1, "Generate", &[2]);
        frames["properties"]["operator"] = json!({"function":"spql.video.frames#packets_slice"});
        Fixture::expressions(
            &mut frames,
            vec![
                Fixture::call(0, "spql.video.frame_slice", &[1, 2, 3]),
                json!({"id":1,"kind":"literal","inputs":[],"display":"0"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"NULL"}),
                json!({"id":3,"kind":"literal","inputs":[],"display":"50"}),
            ],
        );
        let mut decode = Fixture::node(0, "Project", &[1]);
        Fixture::expressions(
            &mut decode,
            vec![
                Fixture::call(0, "spql.video.decode", &[1]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.frame"}),
            ],
        );
        let plan = Fixture::plan(vec![
            decode,
            frames,
            limit,
            video,
            aggregate,
            Fixture::node(5, "Project", &[6]),
            window,
            source,
        ]);
        let graph = WorkflowGraph::from_plan(&plan).unwrap();
        assert_eq!(graph.stages.len(), 6);
        let source = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-7")
            .unwrap();
        assert_eq!(source.content.title, "Read dataset_default");
        assert_eq!(
            source.content.summary,
            "/camera/follower.camera.gripper/video"
        );
        let grouped = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-4")
            .unwrap();
        assert_eq!(grouped.content.title, "Group by episode_id");
        assert_eq!(
            grouped
                .operators
                .iter()
                .map(DiagnosticNode::operator)
                .collect::<Vec<_>>(),
            ["Window", "Project", "Aggregate"]
        );
        let numbering = StageContent::relational(&grouped.operators[0]);
        assert!(numbering.summary.contains("restart for each id"));
        assert!(
            numbering
                .summary
                .contains("chunk_start_offset ascending, sample_ordinal ascending")
        );
        let frames = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-1")
            .unwrap();
        assert_eq!(frames.content.title, "Select video frames");
        assert!(frames.content.summary.contains("Every 50 frame positions"));
        assert!(frames.content.summary.contains("end of the video"));
        assert!(
            graph
                .stages
                .iter()
                .any(|stage| stage.content.summary == "Skip 10 rows, then take 2 rows")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn separate_population_changes_preserve_sources_with_missing_metadata() {
        let mut source = Fixture::node(3, "Source", &[]);
        source["properties"]["operator"] = json!({"provider":"McapTopic"});
        let plan = Fixture::plan(vec![
            Fixture::node(0, "Aggregate", &[1]),
            Fixture::node(1, "Filter", &[2]),
            Fixture::node(2, "Project", &[3]),
            source,
        ]);
        let graph = WorkflowGraph::from_plan(&plan).unwrap();
        // The read keeps its projection; the filter and the aggregate are one
        // shaping stage whose steps the inspector lists.
        assert_eq!(graph.stages.len(), 2);
        let shaped = graph
            .stages
            .iter()
            .find(|stage| stage.id == graph.root)
            .unwrap();
        assert_eq!(shaped.content.title, "Summarize all rows");
        assert_eq!(shaped.footer(&graph.root), "filter · group");
        let source = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-3")
            .unwrap();
        assert_eq!(source.content.title, "Read data");
        assert!(source.content.expression.is_none());
        assert!(
            source
                .content
                .summary
                .contains("source identity is unavailable")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn joined_sources_keep_their_own_dataset_topic_and_revision() {
        let mut camera = Fixture::node(2, "Source", &[]);
        camera["properties"]["operator"] = json!({"provider":"McapTopic", "source":{
            "dataset":{"name":"recording_a", "id":"a", "version_id":"v1"},
            "topic":"/camera", "schema_variant":"video-v1", "data_revision_id":"data-a"
        }});
        let mut gripper = Fixture::node(1, "Source", &[]);
        gripper["properties"]["operator"] = json!({"provider":"McapTopic", "source":{
            "dataset":{"name":"recording_b", "id":"b", "version_id":"v2"},
            "topic":"/gripper", "schema_variant":"joints-v1", "data_revision_id":"data-b"
        }});
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Join", &[2, 1]),
            gripper,
            camera,
        ]))
        .unwrap();
        for (id, name, topic, revision) in [
            ("operator-2", "Read recording_a", "/camera", "data-a"),
            ("operator-1", "Read recording_b", "/gripper", "data-b"),
        ] {
            let source = graph.stages.iter().find(|stage| stage.id == id).unwrap();
            assert_eq!(source.content.title, name);
            assert_eq!(source.content.summary, topic);
            assert!(
                source
                    .content
                    .expression
                    .as_ref()
                    .unwrap()
                    .contains(revision)
            );
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
        assert!(call.content.summary.ends_with("only when needs_review"));
        assert_ne!(graph.root, call.id);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn stage_kinds_follow_native_functions_operators_and_the_root() {
        let mut source = Fixture::node(3, "Source", &[]);
        source["properties"]["operator"] = json!({"provider":"McapTopic"});
        let mut project = Fixture::node(2, "Project", &[3]);
        Fixture::expressions(
            &mut project,
            vec![
                Fixture::call(0, "spql.ai.score", &[1]),
                Fixture::call(1, "spql.image.decode", &[2]),
                json!({"id":2,"kind":"field","inputs":[],"display":"$.image"}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Join", &[1, 1]),
            Fixture::node(1, "Filter", &[2]),
            project,
            source,
        ]))
        .unwrap();
        let kind = |id: &str| {
            graph
                .stages
                .iter()
                .find(|stage| stage.id == id)
                .unwrap()
                .kind(&graph.root)
        };
        assert_eq!(kind("operator-3"), StageKind::Source);
        assert_eq!(kind("call-2-1"), StageKind::Media);
        assert_eq!(kind("call-2-0"), StageKind::Model);
        assert_eq!(kind("operator-1"), StageKind::Shape);
        // A root that combines two populations is still a join; its card
        // carries the Output flag instead of changing kind.
        assert_eq!(kind("operator-0"), StageKind::Join);
        let plain_root = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Filter", &[1]),
            Fixture::node(1, "Source", &[]),
        ]))
        .unwrap();
        assert_eq!(
            plain_root.stages[0].kind(&plain_root.root),
            StageKind::Output
        );
        assert_eq!(StageKind::Output.label(), "Output");
        let join = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Project", &[1]),
            Fixture::node(1, "Join", &[2, 2]),
            Fixture::node(2, "Source", &[]),
        ]))
        .unwrap();
        // The final column selection folds into the join it shapes, so the
        // join is both the combining stage and the workflow's output.
        assert_eq!(join.stages.len(), 2);
        let joined = join
            .stages
            .iter()
            .find(|stage| stage.id == "operator-1")
            .unwrap();
        assert_eq!(joined.kind(&join.root), StageKind::Join);
        assert_eq!(joined.footer(&join.root), "join · select");
        assert_eq!(join.root, "operator-1");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn set_operations_combine_rows_and_linear_filters_read_as_one_condition() {
        let mut union = Fixture::node(0, "SetOp", &[1, 2]);
        union["properties"]["operator"] = json!({"op":"Union","all":true});
        let mut full = Fixture::node(1, "Filter", &[3]);
        Fixture::expressions(
            &mut full,
            vec![
                Fixture::call(0, "spql.eq", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.range"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"\"full\""}),
            ],
        );
        let mut limited = Fixture::node(2, "Filter", &[4]);
        Fixture::expressions(
            &mut limited,
            vec![
                Fixture::call(0, "spql.eq", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.range"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"\"limited\""}),
            ],
        );
        let mut first = Fixture::node(3, "Filter", &[5]);
        Fixture::expressions(
            &mut first,
            vec![
                Fixture::call(0, "spql.eq", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.rn"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"1"}),
            ],
        );
        let mut first_again = Fixture::node(4, "Filter", &[5]);
        Fixture::expressions(
            &mut first_again,
            vec![
                Fixture::call(0, "spql.eq", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.rn"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"1"}),
            ],
        );
        let mut window = Fixture::node(5, "Window", &[6]);
        window["properties"]["operator"] = json!({"expressions":[{"name":"rn","function":{"kind":"RowNumber"},
            "partition_by":["$.id"],"order_by":[{"field":"$log_time","descending":false}]}]});
        let mut earlier = Fixture::node(6, "Filter", &[7]);
        Fixture::expressions(
            &mut earlier,
            vec![
                Fixture::call(0, "spql.gte", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.log_time"}),
                json!({"id":2,"kind":"field","inputs":[],"display":"$.start"}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            union,
            full,
            limited,
            first,
            first_again,
            window,
            earlier,
            Fixture::node(7, "Source", &[]),
        ]))
        .unwrap();
        let combine = graph
            .stages
            .iter()
            .find(|stage| stage.id == graph.root)
            .unwrap();
        assert_eq!(combine.content.title, "Combine rows");
        assert_eq!(
            combine.content.summary,
            "All rows from both inputs, duplicates kept"
        );
        assert_eq!(combine.kind(&graph.root), StageKind::Join);
        assert!(combine.is_set_operation() && !combine.is_join());
        assert_eq!(combine.inputs, ["operator-1", "operator-2"]);
        assert!(
            combine
                .facts()
                .iter()
                .any(|fact| fact.label == "Operation" && fact.value == "Union all")
        );
        // Two consecutive filters are one stage stating both conditions.
        let branch = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-1")
            .unwrap();
        assert_eq!(branch.content.title, "Filter rows");
        assert_eq!(branch.content.summary, "(rn = 1) AND (range = \"full\")");
        assert_eq!(branch.content.summary_kind, SummaryKind::Expression);
        assert_eq!(branch.footer(&graph.root), "filter");
        // A filter that precedes the numbering is not the first-row idea.
        let numbered = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-5")
            .unwrap();
        assert_eq!(numbered.content.title, "Filter rows");
        assert_eq!(numbered.content.summary, "(log_time ≥ start)");
        assert_eq!(numbered.footer(&graph.root), "filter · number");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn reading_folds_its_column_selection_and_one_row_per_group_reads_as_one_idea() {
        // The shape of the live gripper audit: episodes are read and trimmed to
        // two columns through two projections, joined to video frames, numbered
        // within (episode, second), and only the first frame per second kept.
        let mut episodes = Fixture::node(13, "Source", &[]);
        episodes["properties"]["operator"] = json!({"provider":"dataset_member","source":{}});
        let mut trim = Fixture::node(12, "Project", &[13]);
        trim["properties"]["operator"] =
            json!({"expressions":[{"name":"id"},{"name":"message_start_time"}]});
        let mut rename = Fixture::node(11, "Project", &[12]);
        rename["properties"]["operator"] =
            json!({"expressions":[{"name":"id_1"},{"name":"message_start_time"}]});
        rename["output"] = json!({"schema":{"fields":[
            {"name":"id_1","dtype":"i64"},{"name":"message_start_time","dtype":"u64"}
        ]}});
        let mut video = Fixture::node(10, "Source", &[]);
        video["properties"]["operator"] = json!({"provider":"McapVideo","source":{"dataset":{"name":"dataset_default"},"topic":"/camera/gripper/video"}});
        let mut join = Fixture::node(9, "Join", &[10, 11]);
        join["properties"]["operator"] =
            json!({"kind":"Inner","on":[{"left":"$.id","right":"$.id_1"}]});
        let mut window = Fixture::node(7, "Window", &[9]);
        window["properties"]["operator"] = json!({"expressions":[{"name":"__window_expr_0","function":{"kind":"RowNumber"},
            "partition_by":["$.episode_id","$.sampled_second"],
            "order_by":[{"field":"$log_time","descending":false},{"field":"$chunk_start_offset","descending":false}]}]});
        let mut second_rank = Fixture::node(6, "Project", &[7]);
        second_rank["properties"]["operator"] =
            json!({"expressions":[{"name":"episode_id"},{"name":"second_rank"}]});
        let mut first = Fixture::node(5, "Filter", &[6]);
        Fixture::expressions(
            &mut first,
            vec![
                Fixture::call(0, "spql.eq", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.second_rank"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"1"}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            Fixture::node(0, "Project", &[5]),
            first,
            second_rank,
            window,
            join,
            video,
            rename,
            trim,
            episodes,
        ]))
        .unwrap();
        let ids = graph
            .stages
            .iter()
            .map(|stage| stage.id.as_str())
            .collect::<Vec<_>>();
        assert!(!ids.contains(&"operator-12") && !ids.contains(&"operator-11"));
        let read = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-13")
            .unwrap();
        assert_eq!(read.content.title, "Read episodes");
        assert_eq!(
            read.content.summary,
            "One row per dataset episode → id_1, message_start_time"
        );
        assert_eq!(read.kind(&graph.root), StageKind::Source);
        assert_eq!(read.footer(&graph.root), "dataset_member");
        assert_eq!(read.output_columns(), ["id_1", "message_start_time"]);
        let joined = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-9")
            .unwrap();
        assert_eq!(joined.inputs, ["operator-10", "operator-13"]);
        assert_eq!(joined.content.summary, "on id = id_1");
        assert!(
            joined
                .facts()
                .iter()
                .any(|fact| fact.label == "Join" && fact.value == "Inner")
        );
        // The first-per-group idea absorbs the final projection here, so the
        // stage is found by what it says rather than by which operator kept its id.
        let per_second = graph
            .stages
            .iter()
            .find(|stage| stage.content.title.starts_with("One row per"))
            .unwrap();
        assert_eq!(
            per_second.content.title,
            "One row per episode_id, sampled_second"
        );
        assert_eq!(
            per_second.content.summary,
            "First by log_time ascending, chunk_start_offset ascending"
        );
        assert!(
            per_second
                .facts()
                .iter()
                .any(|fact| fact.label == "Condition" && fact.value == "(second_rank = 1)")
        );
        let video = graph
            .stages
            .iter()
            .find(|stage| stage.id == "operator-10")
            .unwrap();
        assert_eq!(video.footer(&graph.root), "McapVideo");
        assert!(
            video
                .facts()
                .iter()
                .any(|fact| fact.label == "Topic" && fact.value == "/camera/gripper/video")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn model_scores_name_their_prompt_and_classifications_list_their_outcomes() {
        let mut project = Fixture::node(0, "Project", &[1]);
        Fixture::expressions(
            &mut project,
            vec![
                Fixture::call(0, "spql.ai.score", &[1, 2]),
                json!({"id":1,"kind":"field","inputs":[],"display":"$.image"}),
                json!({"id":2,"kind":"literal","inputs":[],"display":"\"a robot gripper holding an object\""}),
            ],
        );
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            project,
            Fixture::node(1, "Source", &[]),
        ]))
        .unwrap();
        let score = graph
            .stages
            .iter()
            .find(|stage| stage.content.function.is_some())
            .unwrap();
        assert_eq!(score.content.title, "Score images");
        assert_eq!(
            score.content.summary,
            "“a robot gripper holding an object” · image"
        );
        assert_eq!(score.kind(&graph.root), StageKind::Model);
        assert!(
            score
                .facts()
                .iter()
                .any(|fact| fact.label == "Function" && fact.value == "spql.ai.score")
        );

        let mut decide = Fixture::node(0, "Project", &[1]);
        Fixture::expressions(
            &mut decide,
            vec![
                Fixture::call(0, "spql.case_when", &[1, 2, 3]),
                Fixture::call(1, "spql.gte", &[4, 5]),
                json!({"id":2,"kind":"literal","inputs":[],"display":"\"retained\""}),
                json!({"id":3,"kind":"literal","inputs":[],"display":"\"excluded\""}),
                json!({"id":4,"kind":"field","inputs":[],"display":"$.score"}),
                json!({"id":5,"kind":"literal","inputs":[],"display":"-0.5"}),
            ],
        );
        decide["properties"]["expressions"]["roots"] = json!([{"name":"decision","id":0}]);
        let graph = WorkflowGraph::from_plan(&Fixture::plan(vec![
            decide,
            Fixture::node(1, "Source", &[]),
        ]))
        .unwrap();
        let classify = graph
            .stages
            .iter()
            .find(|stage| stage.id == graph.root)
            .unwrap();
        assert_eq!(classify.content.title, "Classify rows");
        assert_eq!(classify.content.summary, "decision → retained · excluded");
        assert_eq!(classify.content.summary_kind, SummaryKind::Expression);
        assert_eq!(
            classify.content.expression.as_deref(),
            Some("decision = when (score ≥ -0.5): \"retained\"; otherwise \"excluded\"")
        );
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
