//! The pinned object tracker, and what a Track objects node adds to a path.
//!
//! The tracker runs on the Modal service, which tracks what the node names
//! on its camera five times a second and answers every sampled picture with
//! the objects it holds, each by track and box; a fill stores those tracks
//! in the dataset's answers table, and every reading of the node, a preview
//! included, reads them from there. Its step counts each row's objects: the
//! most of them any one picture held. The tracker reads whole episodes, and
//! the node only follows whole episodes, so the count is the episode's.
use crate::activities::curation::document::ObjectTracking;
use crate::activities::curation::path::{
    EpisodeRow, PathError, PathFragment, PathStep, StepSql, StoredAnswers,
};
use serde_json::Value;
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::relations::SpiralQuerySnapshot;

/// The released SAM 3.1 checkpoint on the Modal service, and the service
/// revision whose tracks the answers store. A service that runs another
/// revision refuses the request, so tracks of two revisions never share a
/// column.
pub(in crate::activities::curation) struct ObjectTracker;

impl ObjectTracker {
    const MODEL: &'static str = "facebook/sam3.1@daa63191845a41281374e725f4c9e51c7a824460";
    const REVISION: &'static str = "object-tracking-0.1.0";
}

/// The number a Track objects node measures for each episode: the most
/// objects one sampled picture of it held.
pub(in crate::activities::curation) struct ObjectCount;

impl ObjectCount {
    /// The column a Track objects step writes it to.
    pub(in crate::activities::curation) const COLUMN: &'static str = "object_count";

    /// The count one episode's preview answered in `column`: its row's, or
    /// none when the tracker could not count the episode.
    pub(in crate::activities::curation) fn read(
        answer: &SpiralQuerySnapshot,
        column: &str,
    ) -> Option<u64> {
        answer
            .rows()
            .iter()
            .filter_map(|row| row.get(column).and_then(Value::as_u64))
            .max()
    }
}

impl ObjectTracking {
    /// What the node tracks and on which camera, once both are chosen.
    fn asked(&self) -> Result<(&str, &str), PathError> {
        let camera = self
            .camera()
            .ok_or_else(|| PathError::new("Choose the camera to track objects on."))?;
        if self.prompt().is_empty() {
            return Err(PathError::new("Name what to track, such as “bottle”."));
        }
        Ok((camera, self.prompt()))
    }
}

/// Every episode's tracks are the Modal service's, stored under the
/// checkpoint, the service revision, the camera and what was tracked.
impl StoredAnswers for ObjectTracking {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let (camera, prompt) = self.asked()?;
        AnswerColumn::of(
            "track_objects",
            &[
                "modal.track_objects",
                ObjectTracker::MODEL,
                ObjectTracker::REVISION,
                camera,
                prompt,
            ],
        )
        .map_err(PathError::new)
    }

    fn answer_source(
        &self,
        _dataset: &DatasetSelection,
        _columns: (&AnswerColumn, Option<&AnswerColumn>),
    ) -> Result<AnswerSource, PathError> {
        let (camera, prompt) = self.asked()?;
        Ok(AnswerSource::ObjectTracking {
            camera: camera.to_owned(),
            prompt: prompt.to_owned(),
            model: ObjectTracker::MODEL.to_owned(),
            revision: ObjectTracker::REVISION.to_owned(),
        })
    }
}

impl PathFragment for ObjectTracking {
    fn episode_columns(
        &self,
        row: &mut EpisodeRow,
        column: &AnswerColumn,
    ) -> Result<(), PathError> {
        row.stored(column)
    }

    /// Every row that reaches the node, with the most objects one sampled
    /// picture of its episode held. A row whose episode has no tracks, or
    /// tracks of no picture, carries no count.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let tracks = step.column()?;
        // The tracks must reach the node, and ride on past it, so a node
        // below tracking the same reads them too.
        input.answers().without(tracks)?;
        let name = step.name();
        let counted = input.measuring(name, ObjectCount::COLUMN);
        let count = counted
            .measured(ObjectCount::COLUMN)
            .ok_or_else(|| PathError::new("The count was not measured."))?;
        let carried = |qualifier: &str| {
            input
                .properties()
                .map(|(column, _)| format!(", {qualifier}{column}"))
                .collect::<String>()
        };
        let rides = counted.answers();
        // Every sampled picture of each row's episode, one row each; a row
        // whose episode has no tracks stays, as one row with nothing seen.
        // Each row is then counted over a window and stands for itself once,
        // so what reaches the node is read once.
        let row = "OVER (PARTITION BY episode_id, start_ns, end_ns)";
        let entries = vec![
            format!(
                "{name}_pictures AS (SELECT r.episode_id, r.start_ns, r.end_ns{}{}, \
                 spql.list.len(p.objects) AS visible \
                 FROM {} r LEFT JOIN LATERAL UNNEST(r.{tracks}) p ON TRUE)",
                carried("r."),
                rides.following("r."),
                input.name()
            ),
            format!(
                "{name}_counted AS (SELECT episode_id, start_ns, end_ns{}{}, MAX(visible) {row} AS most, \
                 ROW_NUMBER() {row} AS place FROM {name}_pictures)",
                carried(""),
                rides.following("")
            ),
            format!(
                "{name} AS (SELECT episode_id, start_ns, end_ns{}, CAST(most AS BIGINT) AS {count}{} \
                 FROM {name}_counted WHERE place = 1)",
                carried(""),
                rides.following("")
            ),
        ];
        Ok(StepSql::new(entries, counted))
    }
}
