//! What an exploration node reads from the exploration.
//!
//! Both explorations read a saved subtask table and never rerun annotation,
//! so one snapshot — the parent annotation, its rows, the dataset they were
//! recorded in — is all either of them needs from the payload.
use super::coverage::MeasuredEpisodes;
use crate::activities::workflow::explore::document::{
    ExplorationNode, ExplorationNodeKind, ExplorationPayload, SubtaskAssignment, SubtaskGrouping,
    SubtaskRow, SubtaskSearch,
};
use crate::activities::workflow::explore::review::{EpisodeOrder, SubtaskContext};
use crate::activities::workflow::explore::state::ExplorationController;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

#[derive(Clone, PartialEq)]
pub(super) struct ExplorationSource {
    annotation: Option<String>,
    rows: Vec<SubtaskRow>,
    dataset: Option<DatasetSelection>,
    /// The camera the parent annotation sampled, so a result plays the footage
    /// the description was written from.
    camera: Option<String>,
    /// The episodes the parent annotation actually read, in the order it read
    /// them. An episode it failed on is not one of them.
    measured: Vec<EpisodeId>,
    /// The query a search node remembers; a grouping node has none.
    query: String,
    /// The row that query was taken from, when it came from one the table
    /// still holds.
    asked_about: Option<usize>,
}
impl ExplorationSource {
    pub(super) fn of(controller: ExplorationController, node: &str) -> Self {
        controller.with_payload(|payload| {
            let annotation = payload.source_annotation(node).map(str::to_owned);
            let (camera, measured) = annotation
                .as_deref()
                .map(|source| Self::measurement(payload, source))
                .unwrap_or_default();
            let rows = annotation
                .iter()
                .flat_map(|source| payload.subtasks_for(source).cloned())
                .collect::<Vec<SubtaskRow>>();
            let search = Self::saved_search(payload, node);
            let asked_about = search
                .and_then(SubtaskSearch::source)
                .and_then(|key| rows.iter().position(|row| &row.key() == key));
            Self {
                rows,
                camera,
                measured,
                annotation,
                dataset: payload.dataset().cloned(),
                query: search
                    .map(|search| search.query().to_owned())
                    .unwrap_or_default(),
                asked_about,
            }
        })
    }
    /// What the parent annotation says about its own run: the camera it
    /// sampled and the episodes it completed.
    fn measurement(
        payload: &ExplorationPayload,
        annotation: &str,
    ) -> (Option<String>, Vec<EpisodeId>) {
        match payload.node(annotation).map(ExplorationNode::kind) {
            Some(ExplorationNodeKind::BreakDownSubtasks(settings)) => (
                settings.camera().map(str::to_owned),
                settings.completed_episodes().to_vec(),
            ),
            _ => (None, Vec::new()),
        }
    }
    fn saved_search<'a>(payload: &'a ExplorationPayload, node: &str) -> Option<&'a SubtaskSearch> {
        match payload.node(node).map(|node| node.kind()) {
            Some(ExplorationNodeKind::FindSimilarSubtasks(search)) => Some(search),
            _ => None,
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
    /// Which row the saved query was taken from.
    pub(super) fn asked_about(&self) -> Option<usize> {
        self.asked_about
    }
    /// Everything a view needs to show these rows: the recordings, the camera
    /// they were read from, and the order the episodes list in — the order the
    /// annotation measured them, then any episode only the rows mention.
    pub(super) fn context(&self) -> SubtaskContext {
        SubtaskContext::new(
            self.dataset.clone(),
            self.camera.clone(),
            EpisodeOrder::of(
                self.measured
                    .iter()
                    .chain(self.rows.iter().map(SubtaskRow::episode)),
            ),
        )
    }
    /// The episodes this exploration's answer is measured against: what the
    /// parent annotation read.
    pub(super) fn measured(&self) -> MeasuredEpisodes {
        MeasuredEpisodes::of(&self.measured)
    }
    pub(super) fn descriptions(&self) -> Vec<&str> {
        self.rows.iter().map(SubtaskRow::description).collect()
    }
    /// Where an assignment's row sits in the table it was assigned from.
    fn row_of(&self, assignment: &SubtaskAssignment) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.is(assignment.episode(), assignment.start_time()))
    }
    /// Each row's assignment in `grouping`, in table order; `None` for a row
    /// the grouping has not placed.
    pub(super) fn assignments<'a>(
        &self,
        grouping: &'a SubtaskGrouping,
    ) -> Vec<Option<&'a SubtaskAssignment>> {
        let placed = grouping
            .assignments()
            .iter()
            .map(|assignment| {
                (
                    (assignment.episode(), assignment.start_time().to_bits()),
                    assignment,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        self.rows
            .iter()
            .map(|row| {
                placed
                    .get(&(row.episode(), row.start_time().to_bits()))
                    .copied()
            })
            .collect()
    }
    /// One category's members as an index lists them: in the order given, each
    /// with the row it is and the probability that put it there. An assignment
    /// whose row has since gone is left out rather than drawn as a gap.
    pub(super) fn listed(&self, members: &[&SubtaskAssignment]) -> Vec<(usize, f64)> {
        members
            .iter()
            .filter_map(|member| Some((self.row_of(member)?, member.probability())))
            .collect()
    }
}
