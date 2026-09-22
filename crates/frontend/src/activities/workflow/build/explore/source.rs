//! What an exploration node reads from the Build document.
//!
//! Both explorations read a saved subtask table and never rerun annotation,
//! so one snapshot — the parent annotation, its rows, the dataset they were
//! recorded in — is all either of them needs from the payload.
use crate::activities::workflow::build::document::{
    BuildNodeKind, BuildPayload, SubtaskAssignment, SubtaskRow,
};
use crate::activities::workflow::build::state::BuildController;
use spiral_robotics_common::datasets::DatasetSelection;

#[derive(Clone, PartialEq)]
pub(super) struct ExplorationSource {
    annotation: Option<String>,
    rows: Vec<SubtaskRow>,
    dataset: Option<DatasetSelection>,
    /// The query a search node remembers; a grouping node has none.
    query: String,
}
impl ExplorationSource {
    pub(super) fn of(controller: BuildController, node: &str) -> Self {
        controller.with_payload(|payload| {
            let annotation = payload.source_annotation(node).map(str::to_owned);
            Self {
                rows: annotation
                    .iter()
                    .flat_map(|source| payload.subtasks_for(source).cloned())
                    .collect(),
                annotation,
                dataset: payload.dataset().cloned(),
                query: Self::saved_query(payload, node),
            }
        })
    }
    fn saved_query(payload: &BuildPayload, node: &str) -> String {
        match payload.node(node).map(|node| node.kind()) {
            Some(BuildNodeKind::FindSimilarSubtasks(search)) => search.query().to_owned(),
            _ => String::new(),
        }
    }
    /// The annotation node whose rows this exploration reads, when its
    /// parent is one.
    pub(super) fn annotation(&self) -> Option<&str> {
        self.annotation.as_deref()
    }
    pub(super) fn rows(&self) -> &[SubtaskRow] {
        &self.rows
    }
    pub(super) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(super) fn query(&self) -> &str {
        &self.query
    }
    pub(super) fn descriptions(&self) -> Vec<&str> {
        self.rows.iter().map(SubtaskRow::description).collect()
    }
    /// Where an assignment's row sits in the table it was assigned from.
    pub(super) fn row_of(&self, assignment: &SubtaskAssignment) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.is(assignment.episode(), assignment.start_time()))
    }
}
