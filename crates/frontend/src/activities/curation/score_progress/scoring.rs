//! The progress scorer a Score progress node scores with, and what the node
//! adds to a path.
//!
//! The scorer runs on the Modal service, which scores the scorer's camera of
//! each episode with a velocity for every frame; a fill stores those scores
//! in the dataset's answers table, and every reading of the node, a preview
//! included, reads them from there. Its entries are the scores unnested, one
//! per frame, at the frame's log time with the progress rate it made, and its
//! column folds them into the episode's timeline. The scorer reads whole
//! episodes, so what it is asked never depends on anything above the node.
use crate::activities::curation::document::{ProgressScoring, PublishedProgressModel};
use crate::activities::curation::path::{
    ColumnMeaning, EpisodeRow, ModelEntries, ModelNode, ModelStep, PathError, TableColumn,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::training::PublishedModel;

/// The progress model a node scores with. Its velocity is progress per unit
/// time relative to the expert demonstrations it was trained on; progress is
/// normalized within one episode, never a completion probability.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum ProgressScorer {
    /// The released, task-specific WARP checkpoint.
    Released,
    /// A progress model trained in Studio and published under a name.
    Published(PublishedProgressModel),
}

impl From<&ProgressScoring> for ProgressScorer {
    fn from(scoring: &ProgressScoring) -> Self {
        scoring.published().map_or(Self::Released, |published| {
            Self::Published(published.clone())
        })
    }
}

impl ProgressScorer {
    /// What the request asks the service to score with.
    fn model(&self) -> &str {
        match self {
            Self::Released => {
                "uynitsuj/warp-rm-sim-bottles-sss15@6417be056ed0bc2b2549f7f414fed76d4ca8501f"
            }
            Self::Published(published) => published.model(),
        }
    }
    /// The Modal service revision whose scores the answers store. A service
    /// that runs another one refuses the request, so answers of two
    /// revisions never share a column.
    const fn revision(&self) -> &'static str {
        "warp-progress-0.2.0-fp16"
    }
    /// The camera topic the model was trained on, which it scores. A dataset
    /// that does not record it cannot be scored.
    pub(in crate::activities::curation) fn camera(&self) -> &str {
        match self {
            Self::Released => "/cameras/top_camera-images-rgb",
            Self::Published(published) => published.camera(),
        }
    }
    /// Where the project's published `models` list this scorer, when it is
    /// published and they do.
    pub(super) fn listed_in(&self, models: &[PublishedModel]) -> Option<usize> {
        match self {
            Self::Released => None,
            Self::Published(published) => models
                .iter()
                .position(|model| model.model() == published.model()),
        }
    }
    /// Why a node cannot score with it now: a published model the project's
    /// `models` do not list, or cannot be checked against. `Err(None)` while
    /// they are read.
    pub(super) fn availability(
        &self,
        models: Option<&Result<Vec<PublishedModel>, String>>,
    ) -> Result<(), Option<String>> {
        match (self, models) {
            (Self::Released, _) => Ok(()),
            (Self::Published(_), None) => Err(None),
            (Self::Published(_), Some(Err(error))) => Err(Some(format!(
                "The published progress models could not be read: {error}"
            ))),
            (Self::Published(_), Some(Ok(models))) => match self.listed_in(models) {
                Some(_) => Ok(()),
                None => Err(Some(
                    "This node scores with a model this project has not published; choose another"
                        .to_owned(),
                )),
            },
        }
    }
}

/// Every episode's scores are the Modal service's, stored under the scorer's
/// model, service revision and camera, so each model has its own column.
impl ModelNode for ProgressScoring {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let scorer = ProgressScorer::from(self);
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
        let scorer = ProgressScorer::from(self);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::curation::document::{
        CURATION_ENCODING_VERSION, CurationGraph, CurationNodeKind, DATASET_NODE_ID,
    };
    use crate::activities::curation::path::CurationPath;
    use spiral_robotics_common::training::ModelPublication;

    fn published(name: &str, model: &str) -> PublishedModel {
        PublishedModel::new(
            ModelPublication::new("train-1", 3, name),
            model,
            "/top-camera",
        )
    }

    /// A graph scoring every episode of a dataset with `scoring`, and its
    /// Score progress node.
    fn scored(scoring: ProgressScoring) -> (CurationGraph, String) {
        let mut graph = CurationGraph::new(Some(
            crate::activities::episode::EpisodeListing::mcap_fixture("dataset_bottles"),
        ));
        let node = graph
            .add_child(DATASET_NODE_ID, CurationNodeKind::ScoreProgress(scoring))
            .unwrap();
        (graph, node)
    }

    fn scoring_with(model: &PublishedModel) -> ProgressScoring {
        let mut scoring = ProgressScoring::default();
        scoring.set_published(Some(PublishedProgressModel::from(model)));
        scoring
    }

    /// A document from before published models, without the field, scores
    /// with the released model and saves as it was; one naming a published
    /// model keeps its selector and camera, never its name.
    #[test]
    fn a_document_names_the_published_model_it_scores_with_or_none() {
        let before = r#"{"dataset":null,"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"score_progress-1","kind":{"type":"score_progress"}}],"edges":[{"from":"dataset","to":"score_progress-1"}],"published":[]}"#;
        let graph = CurationGraph::decode(CURATION_ENCODING_VERSION, before).unwrap();
        let CurationNodeKind::ScoreProgress(scoring) =
            graph.node("score_progress-1").unwrap().kind()
        else {
            panic!("a Score progress node");
        };
        assert_eq!(ProgressScorer::from(scoring), ProgressScorer::Released);
        assert_eq!(graph.encode().unwrap(), before);

        let experts = published("Bottle experts", "spiral-robotics-models/0f3c");
        let (graph, _) = scored(scoring_with(&experts));
        let encoded = graph.encode().unwrap();
        let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            json["nodes"][1]["kind"],
            serde_json::json!({
                "type": "score_progress",
                "published": {"model": "spiral-robotics-models/0f3c", "camera": "/top-camera"}
            })
        );
        let decoded = CurationGraph::decode(CURATION_ENCODING_VERSION, &encoded).unwrap();
        assert_eq!(decoded, graph);
    }

    /// Each model scores into its own column, from the camera it was trained
    /// on, and asks the scorer for itself by its selector.
    #[test]
    fn each_model_scores_its_own_camera_into_its_own_column() {
        let experts = published("Bottle experts", "spiral-robotics-models/0f3c");
        let others = published("Any bottle", "spiral-robotics-models/9a8b");
        let released = ProgressScorer::from(&ProgressScoring::default());
        let scorer = ProgressScorer::from(&scoring_with(&experts));
        assert_eq!(released.camera(), "/cameras/top_camera-images-rgb");
        assert_eq!(scorer.camera(), "/top-camera");
        assert_eq!(scorer.model(), "spiral-robotics-models/0f3c");
        assert_eq!(scorer.revision(), released.revision());

        let fills = |scoring: ProgressScoring| {
            let (graph, node) = scored(scoring);
            CurationPath::to(&graph, &node).unwrap().fills().unwrap()
        };
        let [by_released, by_experts, by_others] = [
            ProgressScoring::default(),
            scoring_with(&experts),
            scoring_with(&others),
        ]
        .map(|scoring| fills(scoring).remove(0));
        assert_ne!(by_released.column(), by_experts.column());
        assert_ne!(by_experts.column(), by_others.column());
        assert_eq!(
            by_experts.source(),
            &AnswerSource::ProgressService {
                camera: "/top-camera".to_owned(),
                model: "spiral-robotics-models/0f3c".to_owned(),
                revision: "warp-progress-0.2.0-fp16".to_owned(),
            }
        );
    }

    /// A published model scores once the project's list holds it; the
    /// released one always does.
    #[test]
    fn only_a_listed_published_model_may_score() {
        let experts = published("Bottle experts", "spiral-robotics-models/0f3c");
        let scorer = ProgressScorer::from(&scoring_with(&experts));
        let listed = Ok(vec![
            published("Any bottle", "spiral-robotics-models/9a8b"),
            experts.clone(),
        ]);
        assert_eq!(scorer.availability(Some(&listed)), Ok(()));
        assert_eq!(scorer.listed_in(listed.as_ref().unwrap()), Some(1));
        assert!(matches!(
            scorer.availability(Some(&Ok(Vec::new()))),
            Err(Some(_))
        ));
        assert!(matches!(
            scorer.availability(Some(&Err("offline".into()))),
            Err(Some(_))
        ));
        assert_eq!(scorer.availability(None), Err(None));
        assert_eq!(ProgressScorer::Released.availability(None), Ok(()));
    }
}
