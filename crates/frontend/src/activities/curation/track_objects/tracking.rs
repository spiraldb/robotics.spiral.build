//! The pinned object tracker, and what a Track objects node adds to a path.
//!
//! The tracker runs on the Modal service, which tracks what the node names
//! on its camera at the node's rate and answers every sampled picture with
//! the objects it holds, each by track and box; a fill stores those tracks
//! in the dataset's answers table, and every reading of the node, a preview
//! included, reads them from there. Its entries are the pictures unnested,
//! one per picture at its log time with its objects, and its column folds
//! them into the episode's timeline. The tracker reads whole episodes, so
//! what it is asked never depends on anything above the node.
use crate::activities::curation::document::ObjectTracking;
use crate::activities::curation::path::{
    ColumnMeaning, EpisodeRow, ModelEntries, ModelNode, ModelStep, PathError, TableColumn,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;

/// The released SAM 3.1 checkpoint on the Modal service, and the service
/// revision whose tracks the answers store. A service that runs another
/// revision refuses the request, so tracks of two revisions never share a
/// column.
pub(in crate::activities::curation) struct ObjectTracker;

impl ObjectTracker {
    const MODEL: &'static str = "facebook/sam3.1@daa63191845a41281374e725f4c9e51c7a824460";
    const REVISION: &'static str = "object-tracking-0.2.0";
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
/// checkpoint, the service revision, the camera, what was tracked, and the
/// rate it was sampled at.
impl ModelNode for ObjectTracking {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let (camera, prompt) = self.asked()?;
        AnswerColumn::of(
            "track_objects",
            &serde_json::json!([
                "modal.track_objects",
                ObjectTracker::MODEL,
                ObjectTracker::REVISION,
                camera,
                prompt,
                self.rate(),
            ]),
        )
        .map_err(PathError::new)
    }

    fn answer_source(
        &self,
        _dataset: &DatasetSelection,
        _step: &ModelStep<'_>,
    ) -> Result<AnswerSource, PathError> {
        let (camera, prompt) = self.asked()?;
        Ok(AnswerSource::ObjectTracking {
            camera: camera.to_owned(),
            prompt: prompt.to_owned(),
            rate: self.rate(),
            model: ObjectTracker::MODEL.to_owned(),
            revision: ObjectTracker::REVISION.to_owned(),
        })
    }

    /// Every sampled picture of the episode, at its log time, with the
    /// objects tracked on it. A preview reads only how many it holds; the
    /// boxes are read for the one episode opened.
    fn entries(&self, step: &ModelStep<'_>) -> Result<ModelEntries, PathError> {
        let entry = format!(
            "{} AS (SELECT r.episode_id, r.{} + CAST(p.timestamp_ns AS BIGINT) AS timestamp_ns, \
             p.objects AS objects FROM {} r CROSS JOIN LATERAL UNNEST(r.{}) p)",
            step.entries(),
            EpisodeRow::ORIGIN,
            EpisodeRow::EPISODES,
            step.column()
        );
        Ok(ModelEntries::new(
            vec![entry],
            "timestamp_ns",
            "timestamp_ns, spql.list.len(objects) AS value".to_owned(),
        )
        .folding(
            TableColumn::of_node(step.node(), &step.name(), ColumnMeaning::TrackedObjects),
            (
                "STRUCT_PACK(timestamp_ns := timestamp_ns, objects := objects)".to_owned(),
                None,
            ),
        ))
    }
}
