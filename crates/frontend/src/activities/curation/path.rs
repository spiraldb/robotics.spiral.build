//! A path of the graph compiled to one Spiral statement.
//!
//! Every edge carries one table, one row per episode: `episode_id`, the
//! recorded facts, `kept` — the episode's kept ranges in the recording's
//! clock (MCAP log time) — and the columns the model nodes above added. The
//! statement builds that table in two halves that never read each other's
//! rows. The **reach chain** carries one row per episode reaching each node
//! with its `kept` ranges and nothing else: the dataset starts it, a model
//! node hands on its parent's, and a Select narrows it, each reading the
//! step above it once. The **columns** are folded per episode by each model
//! node from its stored answer, read from the episode row ([`EpisodeRow`]):
//! the node's entries, one row per thing it found ([`ModelEntries`]), folded
//! back into one list per episode. The final `SELECT` joins the reach chain
//! to the facts and every column of the nodes on the path, and the compiler
//! keeps which node wrote each column and what it means ([`TableColumns`]),
//! so a Select, Data, and Training read a column by the node that wrote it.
//!
//! Every model answer is stored: each model node writes one column of the
//! dataset's answers table per exact call ([`AnswerColumn`]), and a fill run
//! computes it for every episode that reaches the node and does not have it
//! yet ([`CurationPath::fills`]), in path order, so the episodes reaching a
//! node are read over the columns above it. The published view reads those
//! columns and asks no model.
//!
//! A preview reads one model node's entries over the stored answers of the
//! episodes it lists, bound as `$1` through `$N`, which it fills first: what
//! the Select's views need of the node, never the node's big lists, each
//! entry with its episode. A Select has no preview; the browser reads its
//! conditions over the previews above it.
mod episode_row;
mod table;

pub(super) use episode_row::EpisodeRow;
pub(super) use table::{ColumnMeaning, TableColumn, TableColumns};

use super::document::{CurationGraph, CurationNode, CurationNodeKind};
use super::graph::DatasetStep;
use super::select::SelectStep;
use crate::activities::episode::EpisodeListing;
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource, ColumnFill, FillEpisodes};
use spiral_robotics_common::datasets::{DatasetSelection, DatasetStreams, EpisodeId};

/// Which reading of a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathSource {
    /// One model node's entries for `episodes` listed episodes, bound as
    /// `$1` through `$N`, reading its stored answers.
    Preview { episodes: usize },
    /// Chained into the view over every episode of the dataset, reading the
    /// stored answers.
    Published,
    /// The one call filling a node's stored column, for the episode bound as
    /// `$1`. A call keeps its function's NULL for an answer it cannot give,
    /// which the fill stores as the episode's failed answer.
    Fill,
}

/// Where one model node's part of a path is written: the node, the column
/// its answers are stored in, and the column of the Break down whose
/// subtasks it judges, when it judges them.
pub(super) struct ModelStep<'a> {
    node: &'a CurationNode,
    column: &'a AnswerColumn,
    subtasks: Option<&'a AnswerColumn>,
}

impl ModelStep<'_> {
    /// The name the node's columns and its entry folding them take.
    pub(super) fn name(&self) -> String {
        self.node.stem()
    }
    /// The entry holding the node's entries; helper entries it needs are
    /// named after the node too.
    pub(super) fn entries(&self) -> String {
        format!("{}_entries", self.name())
    }
    /// The node's id, which names what it writes in the table.
    pub(super) fn node(&self) -> &str {
        self.node.id()
    }
    /// The column the node's answers are stored in.
    pub(super) fn column(&self) -> &AnswerColumn {
        self.column
    }
    /// The column of the Break down whose subtasks the node judges.
    pub(super) fn subtasks(&self) -> Result<&AnswerColumn, PathError> {
        self.subtasks.ok_or_else(|| {
            PathError::new("This node reads the subtasks a Break down above it describes.")
        })
    }
}

/// One table column a model node folds from its entries: the column, each
/// entry's element of its list, and which entries it holds when not all.
struct FoldedColumn {
    column: TableColumn,
    element: String,
    only: Option<String>,
}

/// What one model node adds to a path: its entries, one row per thing it
/// found in an episode and read from the episode row, the last named
/// [`ModelStep::entries`]; what orders an episode's entries; the table
/// columns folding them back per episode; and what a preview reads of each.
pub(super) struct ModelEntries {
    entries: Vec<String>,
    order: &'static str,
    folded: Vec<FoldedColumn>,
    previewed: String,
}

impl ModelEntries {
    pub(super) fn new(entries: Vec<String>, order: &'static str, previewed: String) -> Self {
        Self {
            entries,
            order,
            folded: Vec::new(),
            previewed,
        }
    }
    /// The same entries, folding `column` from `element` of each, or only of
    /// the entries `only` holds for.
    pub(super) fn folding(
        mut self,
        column: TableColumn,
        (element, only): (String, Option<String>),
    ) -> Self {
        self.folded.push(FoldedColumn {
            column,
            element,
            only,
        });
        self
    }
    /// The entry folding every column, one row per episode: each list
    /// collected over the entries in their order, which `spql.list.collect`
    /// keeps (Spiral has no ordered `ARRAY_AGG`).
    fn fold(&self, step: &ModelStep<'_>) -> String {
        let columns = self
            .folded
            .iter()
            .map(|folded| {
                let only = folded
                    .only
                    .as_ref()
                    .map(|only| format!(" FILTER (WHERE {only})"))
                    .unwrap_or_default();
                format!(
                    "spql.list.collect({}){only} AS {}",
                    folded.element,
                    folded.column.name()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} AS (SELECT episode_id, {columns} FROM (SELECT * FROM {} ORDER BY episode_id, {}) ordered GROUP BY episode_id)",
            step.name(),
            step.entries(),
            self.order
        )
    }
}

/// A model node's configuration: the call whose answers it stores, and what
/// it adds to a path from them.
pub(super) trait ModelNode {
    /// The column the node's call stores its answers in. `subtasks` is the
    /// column of the Break down above, for a node that judges its subtasks.
    fn answer_column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError>;
    /// How a fill computes one episode's answer over `dataset`, whose
    /// streams are `streams`.
    fn answer_source(
        &self,
        dataset: &DatasetSelection,
        streams: DatasetStreams<'_>,
        step: &ModelStep<'_>,
    ) -> Result<AnswerSource, PathError>;
    /// The stored answers its entries read, as columns of the episode row.
    fn episode_columns(
        &self,
        row: &mut EpisodeRow<'_>,
        step: &ModelStep<'_>,
    ) -> Result<(), PathError> {
        row.stored(step.column())
    }
    /// Its entries over the episode row, and the columns folding them.
    fn entries(&self, step: &ModelStep<'_>) -> Result<ModelEntries, PathError>;
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

/// A path compiled up to its last node: every `WITH` entry, the reach entry
/// holding `episode_id` and `kept` of every episode it hands on, and the
/// table's columns.
struct CompiledPath {
    entries: Vec<String>,
    reach: String,
    table: TableColumns,
}

/// The nodes from the dataset down to one node.
pub(super) struct CurationPath<'a> {
    graph: &'a CurationGraph,
    nodes: Vec<&'a CurationNode>,
}

impl<'a> CurationPath<'a> {
    pub(super) fn to(graph: &'a CurationGraph, node: &str) -> Result<Self, PathError> {
        let last = graph
            .node(node)
            .ok_or_else(|| PathError::new("That node is no longer in the graph."))?;
        let mut nodes = graph.ancestors(node);
        if nodes.len() > graph.nodes().len() {
            return Err(PathError::new("The graph contains a cycle."));
        }
        nodes.reverse();
        nodes.push(last);
        Ok(Self { graph, nodes })
    }

    /// The path as the one read-only statement a published view is: one row
    /// per episode the last node hands on, with its `kept` ranges, the facts,
    /// and every column of the model nodes on the path.
    pub(super) fn statement(&self) -> Result<String, PathError> {
        let compiled = self.compile()?;
        let reach = compiled.reach.as_str();
        let facts = if reach == DatasetStep::NAME { "r" } else { "d" };
        let mut joins = String::new();
        if reach != DatasetStep::NAME {
            joins.push_str(&format!(
                " JOIN {} d ON d.episode_id = r.episode_id",
                DatasetStep::NAME
            ));
        }
        let mut folded = Vec::<&str>::new();
        let mut columns = vec!["r.episode_id".to_owned(), "r.kept".to_owned()];
        for column in compiled.table.columns() {
            match column.folded_by() {
                None => columns.push(format!("{facts}.{}", column.name())),
                Some(entry) => {
                    if !folded.contains(&entry) {
                        joins.push_str(&format!(
                            " LEFT JOIN {entry} ON {entry}.episode_id = r.episode_id"
                        ));
                        folded.push(entry);
                    }
                    columns.push(format!("{entry}.{}", column.name()));
                }
            }
        }
        Ok(format!(
            "WITH\n  {}\nSELECT {} FROM {reach} r{joins}",
            compiled.entries.join(",\n  "),
            columns.join(", ")
        ))
    }

    /// The table the last node hands on: every column, the node that wrote
    /// it, and what it means.
    pub(super) fn table(&self) -> Result<TableColumns, PathError> {
        self.compile().map(|compiled| compiled.table)
    }

    /// The last node's entries for `episodes` listed episodes, bound as `$1`
    /// through `$N`, over its stored answers: what its preview reads.
    pub(super) fn preview(&self, episodes: usize) -> Result<String, PathError> {
        if episodes == 0 {
            return Err(PathError::new("A preview reads at least one episode."));
        }
        let dataset = self.dataset()?;
        let columns = self.answer_columns()?;
        let depth = self.nodes.len() - 1;
        let (node, stored) = (self.nodes[depth], columns[depth].as_ref());
        let (Some(model), Some(stored)) = (Self::model(node.kind()), stored) else {
            return Err(PathError::new("Only a node that asks a model previews."));
        };
        let step = Self::step(node, stored);
        let mut row = EpisodeRow::new(dataset, PathSource::Preview { episodes });
        model.episode_columns(&mut row, &step)?;
        let built = model.entries(&step)?;
        let mut entries = row.entries();
        entries.extend(built.entries);
        Ok(format!(
            "WITH\n  {}\nSELECT {} FROM {}",
            entries.join(",\n  "),
            built.previewed,
            step.entries()
        ))
    }

    /// The column the last node's answers are stored in, when it asks a
    /// model.
    pub(super) fn answer_column(&self) -> Result<AnswerColumn, PathError> {
        self.answer_columns()?
            .pop()
            .flatten()
            .map(|stored| stored.column)
            .ok_or_else(|| PathError::new("This node asks no model."))
    }

    /// Every column the published view reads, in the order a fill writes
    /// them: for each node that asks a model, the episodes that reach it,
    /// read over the columns above, and how one episode's answer is
    /// computed from `streams`, the dataset's.
    pub(super) fn fills(&self, streams: DatasetStreams<'_>) -> Result<Vec<ColumnFill>, PathError> {
        let columns = self.answer_columns()?;
        let mut fills = Vec::new();
        for (depth, stored) in columns.iter().enumerate() {
            let Some(stored) = stored else {
                continue;
            };
            let reaching = CurationPath {
                graph: self.graph,
                nodes: self.nodes[..depth].to_vec(),
            }
            .reaching()?;
            let step = Self::step(self.nodes[depth], stored);
            fills.push(self.fill(&step, FillEpisodes::Reaching(reaching), streams)?);
        }
        Ok(fills)
    }

    /// The last node's column filled for `episodes`, from `streams`, the
    /// dataset's, as its preview fills it before reading it.
    pub(super) fn fill_for(
        &self,
        episodes: Vec<EpisodeId>,
        streams: DatasetStreams<'_>,
    ) -> Result<ColumnFill, PathError> {
        let depth = self.nodes.len() - 1;
        let columns = self.answer_columns()?;
        let stored = columns[depth]
            .as_ref()
            .ok_or_else(|| PathError::new("This node asks no model."))?;
        let step = Self::step(self.nodes[depth], stored);
        self.fill(&step, FillEpisodes::Listed(episodes), streams)
    }

    fn fill(
        &self,
        step: &ModelStep<'_>,
        episodes: FillEpisodes,
        streams: DatasetStreams<'_>,
    ) -> Result<ColumnFill, PathError> {
        let kind = step.node.kind();
        let model = Self::model(kind).ok_or_else(|| PathError::new("This node asks no model."))?;
        let source = model.answer_source(self.dataset()?, streams, step)?;
        Ok(ColumnFill::new(
            kind.label(),
            step.column.clone(),
            episodes,
            source,
        ))
    }

    /// Every episode the path hands on, read over the stored answers.
    fn reaching(&self) -> Result<String, PathError> {
        let compiled = self.compile()?;
        Ok(format!(
            "WITH\n  {}\nSELECT episode_id FROM {}",
            compiled.entries.join(",\n  "),
            compiled.reach
        ))
    }

    fn dataset(&self) -> Result<&'a DatasetSelection, PathError> {
        Ok(self.listing()?.dataset())
    }

    /// How the dataset lists its episodes, which the path starts from.
    fn listing(&self) -> Result<&'a EpisodeListing, PathError> {
        self.graph
            .listing()
            .ok_or_else(|| PathError::new("Choose the dataset this curation reads."))
    }

    /// Where each node of the path stores its answers, in path order;
    /// nothing for a node that asks no model. A judgment's call names the
    /// Break down above whose subtasks it reads.
    fn answer_columns(&self) -> Result<Vec<Option<NodeColumns>>, PathError> {
        let mut subtasks = None::<AnswerColumn>;
        self.nodes
            .iter()
            .map(|node| {
                let Some(model) = Self::model(node.kind()) else {
                    return Ok(None);
                };
                let column = model.answer_column(subtasks.as_ref())?;
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

    /// The dataset's listing, the episode row of every stored answer the
    /// path reads, each model node's entries and columns, and the reach
    /// chain through every Select.
    fn compile(&self) -> Result<CompiledPath, PathError> {
        let dataset = self.dataset()?;
        let columns = self.answer_columns()?;
        let models = self
            .nodes
            .iter()
            .zip(&columns)
            .filter_map(|(node, stored)| Some((*node, Self::model(node.kind())?, stored.as_ref()?)))
            .collect::<Vec<_>>();
        let mut row = EpisodeRow::new(dataset, PathSource::Published);
        for (node, model, stored) in &models {
            model.episode_columns(&mut row, &Self::step(node, stored))?;
        }
        let mut entries = vec![DatasetStep::new(self.listing()?).entry()];
        entries.extend(row.entries());
        let mut table = TableColumns::facts(self.listing()?);
        let mut reach = DatasetStep::NAME.to_owned();
        for (depth, node) in self.nodes.iter().enumerate() {
            match (
                node.kind(),
                columns[depth].as_ref(),
                Self::model(node.kind()),
            ) {
                (CurationNodeKind::Select(selection), _, _) => {
                    let step = SelectStep::new(node, &reach, self.graph);
                    entries.extend(selection.step(&step)?);
                    reach = node.stem();
                }
                (_, Some(stored), Some(model)) => {
                    let step = Self::step(node, stored);
                    let built = model.entries(&step)?;
                    entries.extend(built.entries.iter().cloned());
                    entries.push(built.fold(&step));
                    table.extend(built.folded.into_iter().map(|folded| folded.column));
                }
                _ => {}
            }
        }
        Ok(CompiledPath {
            entries,
            reach,
            table,
        })
    }

    fn step<'n>(node: &'n CurationNode, stored: &'n NodeColumns) -> ModelStep<'n> {
        ModelStep {
            node,
            column: &stored.column,
            subtasks: stored.subtasks.as_ref(),
        }
    }

    /// The configuration of a node that asks a model, by its kind.
    fn model(kind: &CurationNodeKind) -> Option<&dyn ModelNode> {
        match kind {
            CurationNodeKind::Dataset | CurationNodeKind::Select(_) => None,
            CurationNodeKind::ScoreProgress(scoring) => Some(scoring),
            CurationNodeKind::TrackObjects(tracking) => Some(tracking),
            CurationNodeKind::BreakDownSubtasks(annotation) => Some(annotation),
            CurationNodeKind::FindSimilarSubtasks(search) => Some(search),
            CurationNodeKind::GroupSimilarSubtasks(grouping) => Some(grouping),
        }
    }
}

#[cfg(test)]
mod tests;
