//! A path of the graph compiled to one Spiral statement.
//!
//! Every edge carries episode intervals as a relation: `episode_id`,
//! `start_ns`, `end_ns` in the recording's clock (MCAP log time), then the
//! properties the nodes above measured. Each node kind writes its own step as
//! `WITH` entries over the relation that reaches it ([`PathFragment`]); the
//! path chains the steps from the dataset down and selects the interval and
//! the published properties from the last one.
//!
//! The same statement runs two ways. The preview reads one episode, bound as
//! `$1`; the published view reads the whole dataset whenever it is read. Only
//! the dataset's source differs: it admits the bound episode, and a step that
//! reads a recording's topics directly admits it too, so a preview never
//! reads the rest of the dataset.
use super::document::{CurationGraph, CurationNode, CurationNodeKind};
use spiral_robotics_common::datasets::DatasetSelection;

/// Which reading of a path's statement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathSource {
    /// One episode, bound as `$1`.
    Preview,
    /// Every episode of the dataset.
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
}

/// What one edge carries: the `WITH` entry naming it, the properties after
/// the interval, and which of them the output view shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Relation {
    name: String,
    properties: Vec<&'static str>,
    published: Vec<&'static str>,
}

impl Relation {
    /// A relation carrying `properties`, publishing none of them yet.
    pub(super) fn new(name: impl Into<String>, properties: Vec<&'static str>) -> Self {
        Self {
            name: name.into(),
            properties,
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
    /// The output view shows `property` too.
    pub(super) fn publishing(mut self, property: &'static str) -> Self {
        if !self.published.contains(&property) {
            self.published.push(property);
        }
        self
    }
    /// Every column: the interval, then the properties.
    pub(super) fn columns(&self) -> String {
        Self::interval_and(&self.properties)
    }
    fn interval_and(properties: &[&str]) -> String {
        std::iter::once("episode_id, start_ns, end_ns")
            .chain(properties.iter().copied())
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
            .ok_or_else(|| PathError::Invalid("Only the dataset starts a path.".into()))
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

/// Why a path has no statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PathError {
    /// The Spiral functions the nodes on the path wait for, in path order.
    Waiting(Vec<&'static str>),
    Invalid(String),
}

impl PathError {
    pub(super) fn waiting(function: &'static str) -> Self {
        Self::Waiting(vec![function])
    }
    /// The reason in the words a workspace shows.
    pub(super) fn message(&self) -> String {
        match self {
            Self::Waiting(functions) => format!(
                "Publishing waits for {} in Spiral.",
                functions.join(" and ")
            ),
            Self::Invalid(reason) => reason.clone(),
        }
    }
}

/// The nodes from the dataset down to one node.
pub(super) struct CurationPath<'a> {
    graph: &'a CurationGraph,
    nodes: Vec<&'a CurationNode>,
}

impl<'a> CurationPath<'a> {
    pub(super) fn to(graph: &'a CurationGraph, node: &str) -> Result<Self, PathError> {
        let mut nodes =
            vec![graph.node(node).ok_or_else(|| {
                PathError::Invalid("That node is no longer in the graph.".into())
            })?];
        while let Some(parent) = graph.parent_of(nodes[nodes.len() - 1].id()) {
            if nodes.len() > graph.nodes().len() {
                return Err(PathError::Invalid("The graph contains a cycle.".into()));
            }
            nodes.push(
                graph.node(parent).ok_or_else(|| {
                    PathError::Invalid("An edge starts at an unknown node.".into())
                })?,
            );
        }
        nodes.reverse();
        Ok(Self { graph, nodes })
    }

    /// The path as one read-only statement: the interval of every row the
    /// last node hands on, and the properties it publishes.
    pub(super) fn statement(&self, source: PathSource) -> Result<String, PathError> {
        let dataset = self
            .graph
            .dataset()
            .ok_or_else(|| PathError::Invalid("Choose the dataset this curation reads.".into()))?;
        let mut entries = Vec::new();
        let mut relation = None::<Relation>;
        let mut waiting = Vec::new();
        for (depth, node) in self.nodes.iter().enumerate() {
            let step = PathStep {
                name: format!("{}_{depth}", node.kind().key()),
                input: relation.as_ref(),
                dataset,
                source,
            };
            match Self::step_sql(node.kind(), &step) {
                Ok(written) => {
                    entries.extend(written.entries);
                    relation = Some(written.relation);
                }
                Err(PathError::Waiting(functions)) => {
                    for function in functions {
                        if !waiting.contains(&function) {
                            waiting.push(function);
                        }
                    }
                }
                // Past a node that waits, nothing below has an input; the
                // functions it waits for are the reason.
                Err(_) if !waiting.is_empty() => {}
                Err(error) => return Err(error),
            }
        }
        if !waiting.is_empty() {
            return Err(PathError::Waiting(waiting));
        }
        let relation =
            relation.ok_or_else(|| PathError::Invalid("The path compiled to nothing.".into()))?;
        Ok(format!(
            "WITH\n  {}\nSELECT {} FROM {}",
            entries.join(",\n  "),
            Relation::interval_and(&relation.published),
            relation.name
        ))
    }

    /// One node's step, written by its kind; the kinds Spiral has no
    /// function for yet name the one they wait for.
    fn step_sql(kind: &CurationNodeKind, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        match kind {
            CurationNodeKind::Dataset => step.dataset().step_sql(step),
            CurationNodeKind::Select(selection) => selection.step_sql(step),
            CurationNodeKind::IdentifyUnproductiveMoments(detection) => detection.step_sql(step),
            CurationNodeKind::BreakDownSubtasks(_) => {
                Err(PathError::waiting("spql.ai.breakdown_task"))
            }
            CurationNodeKind::FindSimilarSubtasks(_) => {
                Err(PathError::waiting("spql.ai.similarity"))
            }
            CurationNodeKind::GroupSimilarSubtasks => Err(PathError::waiting("spql.ai.classify")),
        }
    }
}

#[cfg(test)]
mod tests;
