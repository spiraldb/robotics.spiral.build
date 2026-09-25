//! Scoring progress: the pinned scorer, and the step an Identify unproductive
//! moments node adds to a path.
//!
//! The node compiles to `spql.ai.score_progress` over the episode's camera,
//! which answers with a velocity for every frame. Each frame becomes one
//! moment, from its time to the next frame's, cut to the intervals that reach
//! the node. The scorer reads whole episodes, so what it is asked never
//! depends on anything above the node.
use super::moment::Moment;
use crate::activities::curation::document::MomentDetection;
use crate::activities::curation::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use spiral_robotics_common::datasets::CameraVideo;
use spiral_robotics_common::sql::SqlLiteral;

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
    /// The camera topic the checkpoint was trained on. A dataset that does not
    /// record it cannot be scored.
    pub(in crate::activities::curation) const fn camera(self) -> &'static str {
        match self {
            Self::WarpSimBottles => "/cameras/top_camera-images-rgb",
        }
    }
}

/// Every scored frame of every episode that reaches the node, as a moment
/// carrying its pace; a frame the scorer had no support for carries none.
impl PathFragment for MomentDetection {
    fn step_sql(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let (name, from) = (step.name(), input.name());
        let scorer = ProgressScorer::WarpSimBottles;
        let dataset = step.dataset().sql_name();
        let episodes = format!("spql.mcap.episodes({})", SqlLiteral::text(&dataset));
        let pace = Moment::PACE;
        let members = std::iter::once(format!("r.id IN (SELECT episode_id FROM {from})"))
            .chain(step.source().members("r.id"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let videos = CameraVideo::new(&dataset, scorer.camera()).videos(
            (&format!("{name}_packets"), &format!("{name}_videos")),
            &members,
        );
        // Scores are read straight from the videos and unnested before any
        // join, as `score_progress` is read everywhere else; Spiral joins
        // scalar columns, not lists of structs. Spiral has no LEAD or LAG,
        // so the next frame's time is an aggregate over a one-row window.
        let next = "MIN(f.offset_ns) OVER (PARTITION BY f.episode_id ORDER BY f.frame_index ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING)";
        let previous = "MIN(f.offset_ns) OVER (PARTITION BY f.episode_id ORDER BY f.frame_index ROWS BETWEEN 1 PRECEDING AND 1 PRECEDING)";
        let entries = vec![
            videos,
            format!(
                "{name}_scores AS (SELECT id AS episode_id, \
                 spql.ai.score_progress(source, model => {}) AS scores FROM {name}_videos)",
                SqlLiteral::text(scorer.model())
            ),
            format!(
                "{name}_frames AS (SELECT s.episode_id, CAST(p.timestamp_ns AS BIGINT) AS offset_ns, \
                 p.frame_index AS frame_index, \
                 CASE WHEN p.support > 0 AND NOT isnan(p.velocity) THEN p.velocity END AS {pace} \
                 FROM {name}_scores s CROSS JOIN LATERAL UNNEST(s.scores) p)"
            ),
            // A frame lasts until the next one; the last lasts as long as the
            // one before it did. Frame times count from the episode's first
            // message, and the relation carries the recording's clock.
            format!(
                "{name}_moments AS (SELECT f.episode_id, e.origin_ns + f.offset_ns AS start_ns, \
                 e.origin_ns + COALESCE({next}, 2 * f.offset_ns - {previous}) AS end_ns, f.{pace} \
                 FROM {name}_frames f \
                 JOIN (SELECT id, CAST(message_start_time AS BIGINT) AS origin_ns FROM {episodes}) e ON e.id = f.episode_id)"
            ),
            format!(
                "{name} AS (SELECT m.episode_id, GREATEST(m.start_ns, i.start_ns) AS start_ns, \
                 LEAST(m.end_ns, i.end_ns) AS end_ns, m.{pace} \
                 FROM {name}_moments m JOIN {from} i \
                 ON i.episode_id = m.episode_id AND m.start_ns < i.end_ns AND m.end_ns > i.start_ns AND m.end_ns > m.start_ns)"
            ),
        ];
        Ok(StepSql::new(
            entries,
            Relation::new(name, vec![pace]).publishing(pace),
        ))
    }
}
