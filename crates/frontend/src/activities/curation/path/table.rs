//! The curation table's columns as the compiler names them: which node wrote
//! each one and what it holds. A reader of a view is told what a column is
//! from here, never by inspecting its type.
use crate::activities::curation::document::{CategoryId, DATASET_NODE_ID, EpisodeFact};
use crate::activities::episode::EpisodeListing;

/// What one column of the table holds for each episode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) enum ColumnMeaning {
    /// A fact Spiral recorded.
    Fact(EpisodeFact),
    /// A Track objects timeline: `{timestamp_ns, objects}` per picture.
    TrackedObjects,
    /// A Score progress timeline: `{timestamp_ns, progress_rate}` per frame.
    ProgressRates,
    /// A Break down's subtasks: `{description, start_ns, end_ns}`.
    Subtasks,
    /// A Find similar's judgment of every subtask: `{start_ns, end_ns,
    /// similarity}`.
    Similarities,
    /// The subtasks a Group similar put in one category: `{start_ns,
    /// end_ns}`.
    Category(CategoryId),
}

/// One column of the table: its name, the node that wrote it, what it
/// holds, and the entry folding it per episode, which a model node's stem
/// names; a fact is the dataset's own.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct TableColumn {
    name: String,
    node: String,
    meaning: ColumnMeaning,
    folded_by: Option<String>,
}

impl TableColumn {
    /// The column a model node writes: named by the node's `stem`, and by
    /// the category after it for a grouping's.
    pub(in crate::activities::curation) fn of_node(
        node: &str,
        stem: &str,
        meaning: ColumnMeaning,
    ) -> Self {
        let name = match meaning {
            ColumnMeaning::Category(id) => format!("{stem}_category_{id}"),
            _ => stem.to_owned(),
        };
        Self {
            name,
            node: node.to_owned(),
            meaning,
            folded_by: Some(stem.to_owned()),
        }
    }
    pub(in crate::activities::curation) fn name(&self) -> &str {
        &self.name
    }
    /// The id of the node that wrote it.
    pub(in crate::activities::curation) fn node(&self) -> &str {
        &self.node
    }
    pub(in crate::activities::curation) fn meaning(&self) -> ColumnMeaning {
        self.meaning
    }
    /// The entry folding it per episode; none for a fact.
    pub(super) fn folded_by(&self) -> Option<&str> {
        self.folded_by.as_deref()
    }
}

/// Every column of the table at one node, in path order: the facts, then
/// each model node's.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct TableColumns(Vec<TableColumn>);

impl TableColumns {
    /// The facts `listing`'s dataset records, which start every table.
    pub(super) fn facts(listing: &EpisodeListing) -> Self {
        Self(
            EpisodeFact::listed_by(listing)
                .into_iter()
                .map(|fact| TableColumn {
                    name: fact.key().to_owned(),
                    node: DATASET_NODE_ID.to_owned(),
                    meaning: ColumnMeaning::Fact(fact),
                    folded_by: None,
                })
                .collect(),
        )
    }
    pub(in crate::activities::curation) fn columns(&self) -> &[TableColumn] {
        &self.0
    }
    pub(super) fn extend(&mut self, columns: impl IntoIterator<Item = TableColumn>) {
        self.0.extend(columns);
    }
}
