//! The pinned progress scorer, and what a Score progress node adds to a
//! path.
//!
//! The scorer runs on the Modal service, which scores the scorer's camera of
//! each episode with a velocity for every frame; a fill stores those scores
//! in the dataset's answers table, and every reading of the node, a preview
//! included, reads them from there. Its step unnests the scores from every
//! row that reaches the node: each frame becomes one moment, from its time to
//! the next frame's, cut to the interval of the row it arrived on. The
//! scorer reads whole episodes, so what it is asked never depends on
//! anything above the node.
use super::moment::Moment;
use crate::activities::curation::document::ProgressScoring;
use crate::activities::curation::path::{
    EpisodeRow, PathError, PathFragment, PathStep, Relation, StepSql, StoredAnswers,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;

/// The most moments one recording's preview reads: every frame of five and a
/// half minutes at 30 Hz, the Arrow episode query's own bound. A longer
/// recording is refused rather than cut short.
pub(in crate::activities::curation) const MAX_MOMENTS: usize = 10_000;

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
impl StoredAnswers for ProgressScoring {
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
        _columns: (&AnswerColumn, Option<&AnswerColumn>),
    ) -> Result<AnswerSource, PathError> {
        let scorer = ProgressScorer::WarpSimBottles;
        Ok(AnswerSource::ProgressService {
            camera: scorer.camera().to_owned(),
            model: scorer.model().to_owned(),
            revision: scorer.revision().to_owned(),
        })
    }
}

impl PathFragment for ProgressScoring {
    fn episode_columns(
        &self,
        row: &mut EpisodeRow,
        column: &AnswerColumn,
    ) -> Result<(), PathError> {
        row.stored(column)
    }

    /// Every scored frame of every row that reaches the node, as a moment
    /// carrying its progress rate; a frame the scorer had no support for
    /// carries none.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let scores = step.column()?;
        let rides = input.answers().without(scores)?;
        let (name, rate, origin) = (step.name(), Moment::PROGRESS_RATE, EpisodeRow::ORIGIN);
        // Each row that reaches the node brings every frame of its episode,
        // and a frame's next is the next one the same row brought. The frames
        // and the window over them are separate entries.
        let frames =
            "OVER (PARTITION BY episode_id, reached_start_ns, reached_end_ns ORDER BY frame_index)";
        let entries = vec![
            // Frame times count from the episode's first message, and the
            // relation carries the recording's clock.
            format!(
                "{name}_frames AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns{}, \
                 r.{origin} + CAST(p.timestamp_ns AS BIGINT) AS start_ns, p.frame_index AS frame_index, \
                 CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS {rate} \
                 FROM {} r CROSS JOIN LATERAL UNNEST(r.{scores}) p)",
                rides.following("r."),
                input.name()
            ),
            // A frame lasts until the next one; the last lasts as long as the
            // one before it did.
            format!(
                "{name}_moments AS (SELECT episode_id, reached_start_ns, reached_end_ns{}, start_ns, \
                 COALESCE(LEAD(start_ns) {frames}, start_ns + (start_ns - LAG(start_ns) {frames})) AS end_ns, {rate} \
                 FROM {name}_frames)",
                rides.following("")
            ),
            format!(
                "{name} AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, \
                 LEAST(end_ns, reached_end_ns) AS end_ns, {rate}{} FROM {name}_moments \
                 WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns AND end_ns > start_ns)",
                rides.following("")
            ),
        ];
        Ok(StepSql::new(
            entries,
            Relation::new(name, vec![rate])
                .publishing(rate)
                .carrying(rides),
        ))
    }
}
