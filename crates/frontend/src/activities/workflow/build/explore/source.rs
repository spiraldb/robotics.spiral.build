//! What an exploration node reads from the Build document.
//!
//! Both explorations read a saved subtask table and never rerun annotation,
//! so one snapshot — the parent annotation, its rows, the dataset they were
//! recorded in — is all either of them needs from the payload.
use crate::activities::workflow::build::document::{
    BuildNode, BuildNodeKind, BuildPayload, SubtaskAssignment, SubtaskRow,
};
use crate::activities::workflow::build::review::{EpisodeNames, SubtaskContext};
use crate::activities::workflow::build::state::BuildController;
use spiral_robotics_common::datasets::DatasetSelection;

#[derive(Clone, PartialEq)]
pub(super) struct ExplorationSource {
    annotation: Option<String>,
    rows: Vec<SubtaskRow>,
    dataset: Option<DatasetSelection>,
    /// The camera the parent annotation sampled, so a result plays the footage
    /// the description was written from.
    camera: Option<String>,
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
                camera: annotation
                    .as_deref()
                    .and_then(|source| Self::sampled_camera(payload, source)),
                annotation,
                dataset: payload.dataset().cloned(),
                query: Self::saved_query(payload, node),
            }
        })
    }
    fn sampled_camera(payload: &BuildPayload, annotation: &str) -> Option<String> {
        match payload.node(annotation).map(BuildNode::kind) {
            Some(BuildNodeKind::BreakDownSubtasks(settings)) => {
                settings.camera().map(str::to_owned)
            }
            _ => None,
        }
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
    pub(super) fn query(&self) -> &str {
        &self.query
    }
    /// Everything a view needs to show these rows: the recordings, the camera
    /// they were read from, and what to call each episode.
    pub(super) fn context(&self) -> SubtaskContext {
        SubtaskContext::new(
            self.dataset.clone(),
            self.camera.clone(),
            EpisodeNames::of(self.rows.iter().map(SubtaskRow::episode)),
        )
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
