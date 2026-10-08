//! The pinned progress scorer, and what a Score progress node adds to a
//! path.
//!
//! The scorer runs on the Modal service, which scores the scorer's camera of
//! each episode with a velocity for every frame; a fill stores those scores
//! in the dataset's answers table, and every reading of the node, a preview
//! included, reads them from there. Its entries are the scores unnested, one
//! per frame, at the frame's log time with the progress rate it made, and its
//! column folds them into the episode's timeline. The scorer reads whole
//! episodes, so what it is asked never depends on anything above the node.
use crate::activities::curation::document::ProgressScoring;
use crate::activities::curation::path::{
    ColumnMeaning, EpisodeRow, ModelEntries, ModelNode, ModelStep, PathError, TableColumn,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;

/// The released, task-specific WARP checkpoint. Its velocity is progress per
/// unit time relative to expert demonstrations; progress is normalized within
/// one episode, never a completion probability.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) enum ProgressScorer {
    WarpSimBottles,
}

impl ProgressScorer {
    const fn model(self) -> &'static str {
        match self {
            Self::WarpSimBottles => {
                "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
            }
        }
    }
    /// The Modal service revision whose scores the answers store. A service
    /// that runs another one refuses the request, so answers of two
    /// revisions never share a column.
    const fn revision(self) -> &'static str {
        match self {
            Self::WarpSimBottles => "warp-progress-0.2.0-fp16",
        }
    }
    /// The camera topic the checkpoint was trained on. A dataset that does not
    /// record it cannot be scored.
    pub(in crate::activities::curation) const fn camera(self) -> &'static str {
        match self {
            Self::WarpSimBottles => "/cameras/top_camera-images-rgb",
        }
    }
}

/// Every episode's scores are the Modal service's, stored under the scorer's
/// checkpoint, service revision and camera.
impl ModelNode for ProgressScoring {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let scorer = ProgressScorer::WarpSimBottles;
        AnswerColumn::of(
            "score_progress",
            &[
                "modal.score_progress",
                scorer.model(),
                scorer.revision(),
                scorer.camera(),
            ],
        )
        .map_err(PathError::new)
    }

    fn answer_source(
        &self,
        _dataset: &DatasetSelection,
        _step: &ModelStep<'_>,
    ) -> Result<AnswerSource, PathError> {
        let scorer = ProgressScorer::WarpSimBottles;
        Ok(AnswerSource::ProgressService {
            camera: scorer.camera().to_owned(),
            model: scorer.model().to_owned(),
            revision: scorer.revision().to_owned(),
        })
    }

    /// Every scored frame of the episode, at its log time, with its progress
    /// rate; a frame the scorer had no support for has none.
    fn entries(&self, step: &ModelStep<'_>) -> Result<ModelEntries, PathError> {
        let entry = format!(
            "{} AS (SELECT r.episode_id, r.{} + CAST(p.timestamp_ns AS BIGINT) AS timestamp_ns, \
             CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS progress_rate \
             FROM {} r CROSS JOIN LATERAL UNNEST(r.{}) p)",
            step.entries(),
            EpisodeRow::ORIGIN,
            EpisodeRow::EPISODES,
            step.column()
        );
        Ok(ModelEntries::new(
            vec![entry],
            "timestamp_ns",
            "timestamp_ns, progress_rate AS value".to_owned(),
        )
        .folding(
            TableColumn::of_node(step.node(), &step.name(), ColumnMeaning::ProgressRates),
            (
                "STRUCT_PACK(timestamp_ns := timestamp_ns, progress_rate := progress_rate)"
                    .to_owned(),
                None,
            ),
        ))
    }
}
