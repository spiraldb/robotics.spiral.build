//! What a similarity node reads from the curation: the subtasks a Break down
//! above it read that reach the node, and what the node's own preview has
//! judged of them.
use super::coverage::MeasuredEpisodes;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{CurationNode, CurationNodeKind, SubtaskSearch};
use crate::activities::curation::preview::PreviewProgress;
use crate::activities::curation::review::{EpisodeOrder, SubtaskContext};
use crate::activities::curation::state::CurationController;
use spiral_robotics_common::datasets::EpisodeId;

#[derive(Clone, PartialEq)]
pub(super) struct SubtaskSource {
    annotation: Option<String>,
    /// The subtasks of the episodes that reach this node, as the annotation
    /// read them.
    reaching: Vec<SubtaskRow>,
    /// The recordings, the camera the annotation read, and the order the
    /// episodes list in.
    context: SubtaskContext,
    /// The episodes the annotation read of those reaching it, in the order
    /// it read them. An episode it failed on is not one of them.
    measured: Vec<EpisodeId>,
    /// The search a Find similar node asks; a grouping node has none.
    search: Option<SubtaskSearch>,
    /// What the node's own preview judged of those subtasks.
    judged: Vec<SubtaskRow>,
    progress: PreviewProgress,
}

impl SubtaskSource {
    pub(super) fn of(controller: CurationController, node: &str) -> Self {
        let annotation = controller.with_payload(|payload| {
            payload
                .subtask_source(node)
                .and_then(|annotation| payload.node(annotation))
                .cloned()
        });
        let reaching = controller.with_reading(|_, reader| {
            let (Some(reader), Some(annotation)) = (reader, annotation.as_ref()) else {
                return Vec::new();
            };
            reader
                .input(node)
                .episodes()
                .iter()
                .flat_map(|episode| reader.subtasks(annotation.id(), episode))
                .cloned()
                .collect()
        });
        let judged = controller.subtasks(node);
        let measured = annotation
            .as_ref()
            .map(|annotation| Self::measured_by(controller, annotation))
            .unwrap_or_default();
        let camera = annotation
            .as_ref()
            .and_then(|annotation| match annotation.kind() {
                CurationNodeKind::BreakDownSubtasks(settings) => {
                    settings.camera().map(str::to_owned)
                }
                _ => None,
            });
        let (dataset, search) = controller.with_payload(|payload| {
            let search = match payload.node(node).map(CurationNode::kind) {
                Some(CurationNodeKind::FindSimilarSubtasks(search)) => Some(search.clone()),
                _ => None,
            };
            (payload.listing().cloned(), search)
        });
        Self {
            annotation: annotation.map(|annotation| annotation.id().to_owned()),
            context: SubtaskContext::new(
                dataset,
                camera,
                EpisodeOrder::of(
                    measured
                        .iter()
                        .chain(reaching.iter().map(SubtaskRow::episode)),
                ),
            ),
            reaching,
            measured,
            search,
            judged,
            progress: controller.progress(node).unwrap_or_default(),
        }
    }
    /// The episodes the annotation's preview read of those reaching it.
    fn measured_by(controller: CurationController, annotation: &CurationNode) -> Vec<EpisodeId> {
        controller
            .progress(annotation.id())
            .map(|progress| progress.read().to_vec())
            .unwrap_or_default()
    }
    /// The annotation node whose rows this node reads.
    pub(super) fn annotation(&self) -> Option<&str> {
        self.annotation.as_deref()
    }
    pub(super) fn reaching(&self) -> &[SubtaskRow] {
        &self.reaching
    }
    /// The subtasks the node's own preview judged.
    pub(super) fn judged(&self) -> &[SubtaskRow] {
        &self.judged
    }
    /// Where the node's own preview stands.
    pub(super) fn progress(&self) -> &PreviewProgress {
        &self.progress
    }
    /// The query a search node asks.
    pub(super) fn query(&self) -> &str {
        self.search.as_ref().map_or("", SubtaskSearch::query)
    }
    /// The subtask the query was taken from, among those reaching the node.
    pub(super) fn asked_about(&self) -> Option<&SubtaskRow> {
        let key = self.search.as_ref()?.source()?;
        self.reaching.iter().find(|row| key.is(row))
    }
    /// Everything a view needs to show these rows.
    pub(super) fn context(&self) -> SubtaskContext {
        self.context.clone()
    }
    /// The episodes this node's answer is measured against: what the
    /// annotation read.
    pub(super) fn measured(&self) -> MeasuredEpisodes {
        MeasuredEpisodes::of(&self.measured)
    }
}
