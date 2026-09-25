//! A path of the graph compiled to one Spiral statement.
//!
//! Every edge carries episode intervals as a relation: `episode_id`,
//! `start_ns`, `end_ns` in the recording's clock (MCAP log time), then the
//! properties the nodes above measured. Each node kind writes its own step as
//! `WITH` entries over the relation that reaches it ([`PathFragment`]); the
//! path chains the steps from the dataset down and selects the interval and
//! the published properties from the last one.
//!
//! Each step is written once, over whatever relation reaches it, and is read
//! two ways. The published view chains every step from the dataset down into
//! one statement that Spiral runs over the whole dataset whenever it is read.
//! A preview writes only the last node's step, over the rows its parent's
//! preview already holds, supplied to Spiral as `VALUES` ([`StepInput`]), so
//! no model call above the node is made again. A previewed step that reads
//! an episode's recording admits only the episode bound as `$1`, and a model
//! call that cannot answer fails the preview with its reason where the view
//! answers NULL for that row.
use super::document::{CurationGraph, CurationNode, CurationNodeKind};
use spiral_robotics_common::datasets::DatasetSelection;

/// Which reading of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathSource {
    /// Over the rows a parent's preview holds; a step that reads a
    /// recording reads the episode bound as `$1`.
    Preview,
    /// Chained into the view over every episode of the dataset.
    Published,
}

impl PathSource {
    /// The predicate admitting only the previewed episode by `column`, or
    /// nothing when every episode is read.
    pub(super) fn members(self, column: &str) -> Option<String> {
        match self {
            Self::Preview => Some(format!("{column} = $1")),
            Self::Published => None,
        }
    }
    /// The trailing argument of a `spql.ai` call: a preview reads one episode
    /// and shows why it failed, while a view over thousands of episodes must
    /// not die on one, so it keeps the functions' own NULL.
    pub(super) fn on_error(self) -> &'static str {
        match self {
            Self::Preview => ", on_error => 'fail'",
            Self::Published => "",
        }
    }
}

/// One column after the interval: where it is written, and which measure
/// wrote it. Repeated measures on one path write `similarity`,
/// `similarity_2`, and so on.
#[derive(Clone, Debug, PartialEq)]
struct Property {
    column: String,
    measure: &'static str,
}

/// What one edge carries: the `WITH` entry naming it, the properties after
/// the interval, and which of them the output view shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Relation {
    name: String,
    properties: Vec<Property>,
    published: Vec<String>,
}

impl Relation {
    /// A relation carrying `properties`, each its own measure, publishing
    /// none of them yet.
    pub(super) fn new(name: impl Into<String>, properties: Vec<&'static str>) -> Self {
        Self {
            name: name.into(),
            properties: properties
                .into_iter()
                .map(|measure| Property {
                    column: measure.to_owned(),
                    measure,
                })
                .collect(),
            published: Vec::new(),
        }
    }
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    /// The same columns under the name of the step that hands them on.
    pub(super) fn renamed(&self, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..self.clone()
        }
    }
    /// The output view shows `column` too.
    pub(super) fn publishing(mut self, column: &str) -> Self {
        if !self.published.iter().any(|published| published == column) {
            self.published.push(column.to_owned());
        }
        self
    }
    /// These columns under `name`, plus one more `measure` wrote, which the
    /// output view shows. The column is named for the measure, with a suffix
    /// when a node above measured the same.
    pub(super) fn measuring(&self, name: &str, measure: &'static str) -> Self {
        let repeats = self
            .properties
            .iter()
            .filter(|property| property.measure == measure)
            .count();
        let column = match repeats {
            0 => measure.to_owned(),
            repeats => format!("{measure}_{}", repeats + 1),
        };
        let mut measured = self.renamed(name).publishing(&column);
        measured.properties.push(Property { column, measure });
        measured
    }
    /// Every property after the interval, in order: the column and the
    /// measure that wrote it.
    pub(super) fn properties(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.properties
            .iter()
            .map(|property| (property.column.as_str(), property.measure))
    }
    /// The column the nearest node measuring `measure` wrote.
    pub(super) fn measured(&self, measure: &str) -> Option<&str> {
        self.properties
            .iter()
            .rev()
            .find(|property| property.measure == measure)
            .map(|property| property.column.as_str())
    }
    /// Every column: the interval, then the properties.
    pub(super) fn columns(&self) -> String {
        Self::interval_and(
            self.properties
                .iter()
                .map(|property| property.column.as_str()),
        )
    }
    fn interval_and<'a>(properties: impl Iterator<Item = &'a str>) -> String {
        std::iter::once("episode_id, start_ns, end_ns")
            .chain(properties)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What one node adds to a path: the `WITH` entries it defines, in order, and
/// the relation it hands on.
pub(super) struct StepSql {
    entries: Vec<String>,
    relation: Relation,
}

impl StepSql {
    pub(super) fn new(entries: Vec<String>, relation: Relation) -> Self {
        Self { entries, relation }
    }
}

/// Rows supplied to Spiral in place of the path above a node: the `WITH`
/// entries holding them, and the relation they make.
pub(super) struct StepInput {
    entries: Vec<String>,
    relation: Relation,
}

impl StepInput {
    pub(super) fn new(entries: Vec<String>, relation: Relation) -> Self {
        Self { entries, relation }
    }
}

/// Where one node's step is written: the name its relation takes, what
/// reaches it, the dataset the path reads, and which reading this is.
pub(super) struct PathStep<'a> {
    name: String,
    input: Option<&'a Relation>,
    dataset: &'a DatasetSelection,
    source: PathSource,
}

impl PathStep<'_> {
    /// The name of the `WITH` entry holding this step's relation. Helper
    /// entries a step needs are named after it.
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    /// What reaches the node.
    pub(super) fn input(&self) -> Result<&Relation, PathError> {
        self.input
            .ok_or_else(|| PathError::new("Only the dataset starts a path."))
    }
    pub(super) fn dataset(&self) -> &DatasetSelection {
        self.dataset
    }
    pub(super) fn source(&self) -> PathSource {
        self.source
    }
}

/// A node configuration that writes its own step of a path.
pub(super) trait PathFragment {
    fn step_sql(&self, step: &PathStep<'_>) -> Result<StepSql, PathError>;
}

/// Why a path has no statement, in the words a workspace shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PathError(String);

impl PathError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
    pub(super) fn message(&self) -> &str {
        &self.0
    }
}

/// The nodes from the dataset down to one node.
pub(super) struct CurationPath<'a> {
    graph: &'a CurationGraph,
    nodes: Vec<&'a CurationNode>,
}

impl<'a> CurationPath<'a> {
    pub(super) fn to(graph: &'a CurationGraph, node: &str) -> Result<Self, PathError> {
        let mut nodes = vec![
            graph
                .node(node)
                .ok_or_else(|| PathError::new("That node is no longer in the graph."))?,
        ];
        while let Some(parent) = graph.parent_of(nodes[nodes.len() - 1].id()) {
            if nodes.len() > graph.nodes().len() {
                return Err(PathError::new("The graph contains a cycle."));
            }
            nodes.push(
                graph
                    .node(parent)
                    .ok_or_else(|| PathError::new("An edge starts at an unknown node."))?,
            );
        }
        nodes.reverse();
        Ok(Self { graph, nodes })
    }

    /// The path as the one read-only statement a published view is: the
    /// interval of every row the last node hands on, and the properties it
    /// publishes.
    pub(super) fn statement(&self) -> Result<String, PathError> {
        let (entries, relation) = self.compile()?;
        Ok(Self::select(&entries, &relation))
    }

    /// The last node's step alone, over `input`: the statement its preview
    /// runs on the rows its parent's preview holds.
    pub(super) fn preview(&self, input: StepInput) -> Result<String, PathError> {
        let node = self.nodes[self.nodes.len() - 1];
        let step = PathStep {
            name: format!("{}_{}", node.kind().key(), self.nodes.len() - 1),
            input: Some(&input.relation),
            dataset: self.dataset()?,
            source: PathSource::Preview,
        };
        let written = Self::step_sql(node.kind(), &step)?;
        let entries = input
            .entries
            .into_iter()
            .chain(written.entries)
            .collect::<Vec<_>>();
        Ok(Self::select(&entries, &written.relation))
    }

    /// The relation the last node hands on in the published view.
    pub(super) fn relation(&self) -> Result<Relation, PathError> {
        self.compile().map(|(_, relation)| relation)
    }

    fn dataset(&self) -> Result<&'a DatasetSelection, PathError> {
        self.graph
            .dataset()
            .ok_or_else(|| PathError::new("Choose the dataset this curation reads."))
    }

    fn select(entries: &[String], relation: &Relation) -> String {
        format!(
            "WITH\n  {}\nSELECT {} FROM {}",
            entries.join(",\n  "),
            Relation::interval_and(relation.published.iter().map(String::as_str)),
            relation.name
        )
    }

    fn compile(&self) -> Result<(Vec<String>, Relation), PathError> {
        let dataset = self.dataset()?;
        let mut entries = Vec::new();
        let mut relation = None::<Relation>;
        for (depth, node) in self.nodes.iter().enumerate() {
            let step = PathStep {
                name: format!("{}_{depth}", node.kind().key()),
                input: relation.as_ref(),
                dataset,
                source: PathSource::Published,
            };
            let written = Self::step_sql(node.kind(), &step)?;
            entries.extend(written.entries);
            relation = Some(written.relation);
        }
        let relation = relation.ok_or_else(|| PathError::new("The path compiled to nothing."))?;
        Ok((entries, relation))
    }

    /// One node's step, written by its kind.
    fn step_sql(kind: &CurationNodeKind, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        match kind {
            CurationNodeKind::Dataset => step.dataset().step_sql(step),
            CurationNodeKind::Select(selection) => selection.step_sql(step),
            CurationNodeKind::IdentifyUnproductiveMoments(detection) => detection.step_sql(step),
            CurationNodeKind::BreakDownSubtasks(annotation) => annotation.step_sql(step),
            CurationNodeKind::FindSimilarSubtasks(search) => search.step_sql(step),
            CurationNodeKind::GroupSimilarSubtasks(grouping) => grouping.step_sql(step),
        }
    }
}

#[cfg(test)]
mod tests;
