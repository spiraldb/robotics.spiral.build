//! A path of the graph compiled to one Spiral statement.
//!
//! Every edge carries episode intervals as a relation: `episode_id`,
//! `start_ns`, `end_ns` in the recording's clock (MCAP log time), then the
//! properties the nodes above measured. A node adds to a path in two parts
//! ([`PathFragment`]). What it asks a model about each whole episode is a
//! column of that episode's row ([`EpisodeRow`]). Its step then works over
//! the interval rows that reach it: the node that asked the episode unnests
//! the answer and cuts it to the interval of the row it arrived on, and a
//! judgment of each subtask reads its answer at the subtask's place in the
//! Break down it came from. A node's rule, when it keeps by one, filters what
//! its step wrote; a Select is a rule alone.
//!
//! Every model answer is stored: each node that asks a model writes one
//! column of the dataset's answers table per exact call ([`AnswerColumn`]),
//! and a fill run computes it for every episode that reaches the node and
//! does not have it yet ([`CurationPath::fills`]), in path order, so the
//! episodes reaching a node are read over the columns above it. The
//! published view reads those columns and asks no model: where the path
//! leaves whole episodes, each episode's row is built once, joined to the
//! answers table, and the answers ride on the interval rows until the step
//! that unnests them. Every step reads the one before it exactly once, so
//! Spiral, which pastes a `WITH` entry into every place that reads it, reads
//! each answer once.
//!
//! A preview writes only the last node, over the rows its parent's preview
//! already holds, supplied to Spiral as `VALUES` ([`StepInput`]): the node's
//! own row for the episode bound as `$1` alone, joined to those rows, then
//! the same step, reading the node's stored column, which the preview fills
//! first for the episodes it reads. The preview leaves the node's rule out:
//! the browser keeps by it over what the step read, so moving the rule's
//! threshold asks nothing again.
mod episode_row;

pub(super) use episode_row::EpisodeRow;

use super::document::{CurationGraph, CurationNode, CurationNodeKind, PopulationKind};
use episode_row::RowAnswers;
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource, ColumnFill, FillEpisodes};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

/// Which reading of a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathSource {
    /// Over the rows a parent's preview holds; the node's own episode row
    /// reads the stored answers of the episode bound as `$1`.
    Preview,
    /// Chained into the view over every episode of the dataset, reading the
    /// stored answers.
    Published,
    /// The one call filling a node's stored column, for the episode bound as
    /// `$1`. A call keeps its function's NULL for an answer it cannot give,
    /// which the fill stores as the episode's failed answer.
    Fill,
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
    /// The same rows as a preview holds them: no episode answer rides on.
    pub(super) fn without_answers(&self) -> Self {
        self.clone().carrying(RowAnswers::default())
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
/// reaches it, and the column its answers are stored in when it asks a
/// model.
pub(super) struct PathStep<'a> {
    name: String,
    input: Option<&'a Relation>,
    column: Option<&'a AnswerColumn>,
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
    /// The column the node's answers are stored in.
    pub(super) fn column(&self) -> Result<&AnswerColumn, PathError> {
        self.column
            .ok_or_else(|| PathError::new("This node asks no model."))
    }
}

/// A node configuration that writes its own part of a path.
pub(super) trait PathFragment {
    /// What the node asks a model about each whole episode, as columns of
    /// its row, stored in `column`. Most nodes ask nothing.
    fn episode_columns(
        &self,
        _row: &mut EpisodeRow,
        _column: &AnswerColumn,
    ) -> Result<(), PathError> {
        Ok(())
    }
    /// Its step over the interval rows that reach it.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError>;
}

/// A node configuration that asks a model, whose answers are stored: one
/// column of the dataset's answers table per exact call.
pub(super) trait StoredAnswers {
    /// The column the node's call stores its answers in. `subtasks` is the
    /// column of the Break down above, for a node that judges its subtasks.
    fn answer_column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError>;
    /// How a fill computes one episode's answer over `dataset`: the node's
    /// column, and the Break down column it judges, as `answer_column` had.
    fn answer_source(
        &self,
        dataset: &DatasetSelection,
        columns: (&AnswerColumn, Option<&AnswerColumn>),
    ) -> Result<AnswerSource, PathError>;
}

/// Where one node of a path stores its answers: its column, and the column
/// of the Break down whose subtasks it judges, when it judges them.
#[derive(Clone)]
struct NodeColumns {
    column: AnswerColumn,
    subtasks: Option<AnswerColumn>,
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
        let published = compiled.relation.published.iter().map(String::as_str);
        Ok(Self::select(
            &compiled.entries,
            &compiled.relation,
            published,
        ))
    }

    /// The last node's step alone, over `input`, without its rule: the
    /// statement its preview runs on the rows its parent's preview holds,
    /// with the node's own episode row beside them. It answers every
    /// property, not only those the view shows, since the next node's
    /// preview is handed them all.
    pub(super) fn preview(&self, input: StepInput) -> Result<String, PathError> {
        let depth = self.nodes.len() - 1;
        let dataset = self.dataset()?;
        let fragment = Self::fragment(self.nodes[depth].kind(), dataset)
            .ok_or_else(|| PathError::new("Only a measurement previews."))?;
        let columns = self.answer_columns()?;
        let column = columns[depth].as_ref().map(|stored| &stored.column);
        let mut row = EpisodeRow::new(dataset, PathSource::Preview);
        if let Some(column) = column {
            fragment.episode_columns(&mut row, column)?;
        }
        let mut entries = input.entries;
        let reached = match row.over(&input.relation)? {
            Some(built) => {
                entries.extend(built.entries);
                built.relation
            }
            None => input.relation,
        };
        let written = fragment.interval_step(&PathStep {
            name: self.step_name(depth),
            input: Some(&reached),
            column,
        })?;
        entries.extend(written.entries);
        let properties = written.relation.properties().map(|(column, _)| column);
        Ok(Self::select(&entries, &written.relation, properties))
    }

    /// The relation the last node hands on in the published view.
    pub(super) fn relation(&self) -> Result<Relation, PathError> {
        self.compile().map(|compiled| compiled.relation)
    }

    /// Every column the published view reads, in the order a fill writes
    /// them: for each node that asks a model, the episodes that reach it,
    /// read over the columns above, and how one episode's answer is
    /// computed.
    pub(super) fn fills(&self) -> Result<Vec<ColumnFill>, PathError> {
        let columns = self.answer_columns()?;
        let mut fills = Vec::new();
        for (depth, stored) in columns.iter().enumerate() {
            let Some(stored) = stored else {
                continue;
            };
            let node = self.nodes[depth];
            let reaching = CurationPath {
                graph: self.graph,
                nodes: self.nodes[..depth].to_vec(),
            }
            .reaching()?;
            fills.push(self.fill(node, stored, FillEpisodes::Reaching(reaching))?);
        }
        Ok(fills)
    }

    /// The last node's column filled for `episodes`, as its preview fills
    /// it before reading it.
    pub(super) fn fill_for(&self, episodes: Vec<EpisodeId>) -> Result<ColumnFill, PathError> {
        let depth = self.nodes.len() - 1;
        let columns = self.answer_columns()?;
        let stored = columns[depth]
            .as_ref()
            .ok_or_else(|| PathError::new("This node asks no model."))?;
        self.fill(self.nodes[depth], stored, FillEpisodes::Listed(episodes))
    }

    fn fill(
        &self,
        node: &CurationNode,
        stored: &NodeColumns,
        episodes: FillEpisodes,
    ) -> Result<ColumnFill, PathError> {
        let answers =
            Self::stored(node.kind()).ok_or_else(|| PathError::new("This node asks no model."))?;
        let source =
            answers.answer_source(self.dataset()?, (&stored.column, stored.subtasks.as_ref()))?;
        Ok(ColumnFill::new(
            node.kind().label(),
            stored.column.clone(),
            episodes,
            source,
        ))
    }

    /// Every episode the path hands on, once each, read over the stored
    /// answers.
    fn reaching(&self) -> Result<String, PathError> {
        let compiled = self.compile()?;
        Ok(format!(
            "WITH\n  {}\nSELECT DISTINCT episode_id FROM {}",
            compiled.entries.join(",\n  "),
            compiled.relation.name
        ))
    }

    fn dataset(&self) -> Result<&'a DatasetSelection, PathError> {
        self.graph
            .dataset()
            .ok_or_else(|| PathError::new("Choose the dataset this curation reads."))
    }

    fn step_name(&self, depth: usize) -> String {
        format!("{}_{depth}", self.nodes[depth].kind().key())
    }

    /// The interval and `columns` of every row of `relation`.
    fn select<'c>(
        entries: &[String],
        relation: &Relation,
        columns: impl Iterator<Item = &'c str>,
    ) -> String {
        format!(
            "WITH\n  {}\nSELECT {} FROM {}",
            entries.join(",\n  "),
            Relation::interval_and(columns),
            relation.name
        )
    }

    /// Where each node of the path stores its answers, in path order;
    /// nothing for a node that asks no model. A judgment's call names the
    /// Break down above whose subtasks it reads.
    fn answer_columns(&self) -> Result<Vec<Option<NodeColumns>>, PathError> {
        let mut subtasks = None::<AnswerColumn>;
        self.nodes
            .iter()
            .map(|node| {
                let Some(answers) = Self::stored(node.kind()) else {
                    return Ok(None);
                };
                let column = answers.answer_column(subtasks.as_ref())?;
                let stored = NodeColumns {
                    column: column.clone(),
                    subtasks: subtasks.clone(),
                };
                if matches!(node.kind(), CurationNodeKind::BreakDownSubtasks(_)) {
                    subtasks = Some(column);
                }
                Ok(Some(stored))
            })
            .collect()
    }

    /// Every node's step from the dataset down, each followed by its rule,
    /// with the episode row built once, before the first node below the
    /// dataset's own Select: until then every row is a whole episode.
    fn compile(&self) -> Result<StepSql, PathError> {
        let dataset = self.dataset()?;
        let columns = self.answer_columns()?;
        let column = |depth: usize| columns[depth].as_ref().map(|stored| &stored.column);
        let mut row = EpisodeRow::new(dataset, PathSource::Published);
        for (depth, node) in self.nodes.iter().enumerate() {
            if let (Some(fragment), Some(column)) =
                (Self::fragment(node.kind(), dataset), column(depth))
            {
                fragment.episode_columns(&mut row, column)?;
            }
        }
        let mut row = Some(row);
        let mut entries = Vec::new();
        let mut relation = None::<Relation>;
        for (depth, node) in self.nodes.iter().enumerate() {
            let whole_episodes =
                self.graph.population_kind(node.id()) == Some(PopulationKind::Episodes);
            if !whole_episodes
                && let Some(episodes) = relation.as_ref()
                && let Some(built) = row
                    .take()
                    .map(|row| row.over(episodes))
                    .transpose()?
                    .flatten()
            {
                entries.extend(built.entries);
                relation = Some(built.relation);
            }
            let name = self.step_name(depth);
            // A measure's rule keeps from what its own step wrote, under a
            // name of its own; a Select's rule is its whole step.
            let mut rule_name = name.clone();
            if let Some(fragment) = Self::fragment(node.kind(), dataset) {
                let written = fragment.interval_step(&PathStep {
                    name,
                    input: relation.as_ref(),
                    column: column(depth),
                })?;
                entries.extend(written.entries);
                relation = Some(written.relation);
                rule_name.push_str("_kept");
            }
            if let Some(rule) = node.keeps() {
                let kept = rule.interval_step(&PathStep {
                    name: rule_name,
                    input: relation.as_ref(),
                    column: None,
                })?;
                entries.extend(kept.entries);
                relation = Some(kept.relation);
            }
        }
        let relation = relation.ok_or_else(|| PathError::new("The path compiled to nothing."))?;
        Ok(StepSql::new(entries, relation))
    }

    /// The configuration that writes a node's own step, by its kind; a
    /// Select asks nothing, and its rule is all it writes.
    fn fragment<'k>(
        kind: &'k CurationNodeKind,
        dataset: &'k DatasetSelection,
    ) -> Option<&'k dyn PathFragment> {
        match kind {
            CurationNodeKind::Dataset => Some(dataset),
            CurationNodeKind::Select => None,
            CurationNodeKind::ScoreProgress(scoring) => Some(scoring),
            CurationNodeKind::BreakDownSubtasks(annotation) => Some(annotation),
            CurationNodeKind::FindSimilarSubtasks(search) => Some(search),
            CurationNodeKind::GroupSimilarSubtasks(grouping) => Some(grouping),
        }
    }

    /// The configuration of a node that asks a model, by its kind.
    fn stored(kind: &CurationNodeKind) -> Option<&dyn StoredAnswers> {
        match kind {
            CurationNodeKind::Dataset | CurationNodeKind::Select => None,
            CurationNodeKind::ScoreProgress(scoring) => Some(scoring),
            CurationNodeKind::BreakDownSubtasks(annotation) => Some(annotation),
            CurationNodeKind::FindSimilarSubtasks(search) => Some(search),
            CurationNodeKind::GroupSimilarSubtasks(grouping) => Some(grouping),
        }
    }
}

#[cfg(test)]
mod tests;
