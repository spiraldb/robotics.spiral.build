//! A path of the graph compiled to one Spiral statement.
//!
//! Every edge carries episode intervals as a relation: `episode_id`,
//! `start_ns`, `end_ns` in the recording's clock (MCAP log time), then the
//! properties the nodes above measured. A node adds to a path in two parts
//! ([`PathFragment`]). What it asks a model about each whole episode is a
//! column of that episode's row ([`EpisodeRow`]), beside the camera videos
//! and the instruction the call reads. Its step then works over the interval
//! rows that reach it: a Select filters them, a judgment adds a column, and
//! the node that asked the episode unnests the answer and cuts it to the
//! interval of the row it arrived on.
//!
//! Each step is written once and read two ways. The published view is one
//! stream of rows over every episode of the dataset. Where the path leaves
//! whole episodes, each episode's row is built once with every answer the
//! nodes below ask for, and the answers ride on the interval rows until the
//! step that unnests them. Every step reads the one before it exactly once,
//! so Spiral, which pastes a `WITH` entry into every place that reads it,
//! never asks a model twice. A preview writes only the last node, over the
//! rows its parent's preview already holds, supplied to Spiral as `VALUES`
//! ([`StepInput`]): the node's own row for the episode bound as `$1` alone,
//! joined to those rows, then the same step. No model call above the node is
//! made again, and one that cannot answer fails the preview with its reason
//! where the view answers NULL for that row.
mod episode_row;

pub(super) use episode_row::EpisodeRow;

use super::document::{CurationGraph, CurationNode, CurationNodeKind, PopulationKind};
use episode_row::RowAnswers;
use spiral_robotics_common::datasets::DatasetSelection;

/// Which reading of a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathSource {
    /// Over the rows a parent's preview holds; the node's own episode row
    /// reads the episode bound as `$1`.
    Preview,
    /// Chained into the view over every episode of the dataset.
    Published,
}

impl PathSource {
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
/// the interval, which of them the output view shows, and the answers riding
/// on for the steps below.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Relation {
    name: String,
    properties: Vec<Property>,
    published: Vec<String>,
    answers: RowAnswers,
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
            answers: RowAnswers::default(),
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
    /// These columns, with `answers` riding on for the steps below.
    pub(super) fn carrying(mut self, answers: RowAnswers) -> Self {
        self.answers = answers;
        self
    }
    /// The answers riding on these rows.
    pub(super) fn answers(&self) -> &RowAnswers {
        &self.answers
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
    /// Every column: the interval, the properties, then the answers riding
    /// on.
    pub(super) fn columns(&self) -> String {
        self.columns_of("")
    }
    /// The same, each after `qualifier`.
    fn columns_of(&self, qualifier: &str) -> String {
        let interval = ["episode_id", "start_ns", "end_ns"].map(String::from);
        let properties = self
            .properties
            .iter()
            .map(|property| property.column.clone());
        let answers = self.answers.columns().map(String::from);
        interval
            .into_iter()
            .chain(properties)
            .chain(answers)
            .map(|column| format!("{qualifier}{column}"))
            .collect::<Vec<_>>()
            .join(", ")
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
/// reaches it, and which reading this is.
pub(super) struct PathStep<'a> {
    name: String,
    input: Option<&'a Relation>,
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
    pub(super) fn source(&self) -> PathSource {
        self.source
    }
}

/// A node configuration that writes its own part of a path.
pub(super) trait PathFragment {
    /// What the node asks a model about each whole episode, as columns of
    /// its row. Most nodes ask nothing.
    fn episode_columns(&self, _row: &mut EpisodeRow) -> Result<(), PathError> {
        Ok(())
    }
    /// Its step over the interval rows that reach it.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError>;
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
        let compiled = self.compile()?;
        Ok(Self::select(&compiled.entries, &compiled.relation))
    }

    /// The last node alone, over `input`: the statement its preview runs on
    /// the rows its parent's preview holds, with the node's own episode row
    /// beside them.
    pub(super) fn preview(&self, input: StepInput) -> Result<String, PathError> {
        let depth = self.nodes.len() - 1;
        let dataset = self.dataset()?;
        let fragment = Self::fragment(self.nodes[depth].kind(), dataset);
        let mut row = EpisodeRow::new(dataset, PathSource::Preview);
        fragment.episode_columns(&mut row)?;
        let mut entries = input.entries;
        let reached = match row.over(&input.relation) {
            Some(built) => {
                entries.extend(built.entries);
                built.relation
            }
            None => input.relation,
        };
        let written = fragment.interval_step(&PathStep {
            name: self.step_name(depth),
            input: Some(&reached),
            source: PathSource::Preview,
        })?;
        entries.extend(written.entries);
        Ok(Self::select(&entries, &written.relation))
    }

    /// The relation the last node hands on in the published view.
    pub(super) fn relation(&self) -> Result<Relation, PathError> {
        self.compile().map(|compiled| compiled.relation)
    }

    fn dataset(&self) -> Result<&'a DatasetSelection, PathError> {
        self.graph
            .dataset()
            .ok_or_else(|| PathError::new("Choose the dataset this curation reads."))
    }

    fn step_name(&self, depth: usize) -> String {
        format!("{}_{depth}", self.nodes[depth].kind().key())
    }

    fn select(entries: &[String], relation: &Relation) -> String {
        format!(
            "WITH\n  {}\nSELECT {} FROM {}",
            entries.join(",\n  "),
            Relation::interval_and(relation.published.iter().map(String::as_str)),
            relation.name
        )
    }

    /// Every node's step from the dataset down, with the episode row built
    /// once, before the first node below the dataset's own Selects: until
    /// then every row is a whole episode.
    fn compile(&self) -> Result<StepSql, PathError> {
        let dataset = self.dataset()?;
        let mut row = EpisodeRow::new(dataset, PathSource::Published);
        for node in &self.nodes {
            Self::fragment(node.kind(), dataset).episode_columns(&mut row)?;
        }
        let mut row = Some(row);
        let mut entries = Vec::new();
        let mut relation = None::<Relation>;
        for (depth, node) in self.nodes.iter().enumerate() {
            let whole_episodes =
                self.graph.population_kind(node.id()) == Some(PopulationKind::Episodes);
            if !whole_episodes
                && let Some(built) = relation
                    .as_ref()
                    .and_then(|episodes| row.take()?.over(episodes))
            {
                entries.extend(built.entries);
                relation = Some(built.relation);
            }
            let written = Self::fragment(node.kind(), dataset).interval_step(&PathStep {
                name: self.step_name(depth),
                input: relation.as_ref(),
                source: PathSource::Published,
            })?;
            entries.extend(written.entries);
            relation = Some(written.relation);
        }
        let relation = relation.ok_or_else(|| PathError::new("The path compiled to nothing."))?;
        Ok(StepSql::new(entries, relation))
    }

    /// The configuration that writes a node's part, by its kind.
    fn fragment<'k>(
        kind: &'k CurationNodeKind,
        dataset: &'k DatasetSelection,
    ) -> &'k dyn PathFragment {
        match kind {
            CurationNodeKind::Dataset => dataset,
            CurationNodeKind::Select(selection) => selection,
            CurationNodeKind::IdentifyUnproductiveMoments(detection) => detection,
            CurationNodeKind::BreakDownSubtasks(annotation) => annotation,
            CurationNodeKind::FindSimilarSubtasks(search) => search,
            CurationNodeKind::GroupSimilarSubtasks(grouping) => grouping,
        }
    }
}

#[cfg(test)]
mod tests;
