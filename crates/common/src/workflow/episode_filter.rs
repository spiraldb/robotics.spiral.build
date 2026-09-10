//! Native episode-filter authoring. All persisted meaning lives in graph nodes.
use super::{
    WorkflowEdgeSpec, WorkflowNodeConfig, WorkflowNodeSpec, WorkflowSpec, WorkflowValidationIssue,
    WorkflowValidationReport,
};
use crate::datasets::{DatasetSelection, DatasetSummary, TopicRevision};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraBinding {
    topic: String,
    revision: TopicRevision,
    field: Vec<String>,
}
impl CameraBinding {
    pub fn new(topic: String, revision: TopicRevision) -> Self {
        Self {
            topic,
            revision,
            field: vec!["value".into(), "data".into()],
        }
    }
    pub fn topic(&self) -> &str {
        &self.topic
    }
    pub const fn revision(&self) -> &TopicRevision {
        &self.revision
    }
    pub fn field(&self) -> &[String] {
        &self.field
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraScoring {
    camera: CameraBinding,
    producer: CameraProducer,
    concept: String,
    interval_ms: u32,
}
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CameraProducer {
    #[serde(rename = "siglip-base")]
    SiglipBase,
}
impl CameraScoring {
    pub fn new(camera: CameraBinding, concept: String) -> Self {
        Self {
            camera,
            producer: CameraProducer::SiglipBase,
            concept,
            interval_ms: 1000,
        }
    }
    pub const fn camera(&self) -> &CameraBinding {
        &self.camera
    }
    pub fn concept(&self) -> &str {
        &self.concept
    }
    pub const fn interval_ms(&self) -> u32 {
        self.interval_ms
    }
    pub fn description(&self) -> String {
        format!(
            "Score {} with pinned SigLIP for “{}”. Sample the first frame in each {} ms interval, measured from episode start.",
            self.camera.topic, self.concept, self.interval_ms
        )
    }
    fn validate(&self) -> Result<(), String> {
        if self.camera.topic.trim().is_empty()
            || self.camera.topic.len() > 512
            || self.camera.revision.version().is_empty()
            || self.camera.revision.variant().is_empty()
            || self.camera.field != ["value", "data"]
        {
            return Err("Choose a camera binding returned by dataset inspection.".into());
        }
        if self.concept.trim().is_empty() || self.concept.chars().count() > 80 {
            return Err("The SigLIP concept needs 1–80 characters.".into());
        }
        if !(1..=60_000).contains(&self.interval_ms) {
            return Err("The sampling interval must be between 1 and 60,000 ms.".into());
        }
        Ok(())
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EpisodeAggregation {
    Any,
    MoreThanPercent { percent: u8 },
}
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownPolicy {
    Exclude,
    Keep,
}
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeFilterRule {
    score_above_percent: u8,
    exclude_when: EpisodeAggregation,
    unknown: UnknownPolicy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpisodeDecision {
    Included,
    Excluded,
    Unknown,
}
impl EpisodeFilterRule {
    pub const fn new(score_above_percent: u8, exclude_when: EpisodeAggregation) -> Self {
        Self {
            score_above_percent,
            exclude_when,
            unknown: UnknownPolicy::Exclude,
        }
    }
    pub fn description(&self) -> String {
        let fraction = match self.exclude_when {
            EpisodeAggregation::Any => "any sampled frame has".to_owned(),
            EpisodeAggregation::MoreThanPercent { percent } => {
                format!("more than {percent}% of sampled frames have")
            }
        };
        format!(
            "Exclude an episode when {fraction} a score above {}. Episodes with missing camera data, empty sampling intervals, or failed scores are unknown and {} the output.",
            f32::from(self.score_above_percent) / 100.0,
            match self.unknown {
                UnknownPolicy::Exclude => "excluded from",
                UnknownPolicy::Keep => "kept in",
            }
        )
    }
    fn validate(&self) -> Result<(), String> {
        if self.score_above_percent > 100
            || matches!(self.exclude_when, EpisodeAggregation::MoreThanPercent { percent } if percent >= 100)
        {
            return Err(
                "Score threshold must be 0–100%; episode fraction must be below 100%.".into(),
            );
        }
        Ok(())
    }
    /// Output membership is separate from the evidence decision so retained unknowns
    /// never appear to have passed the filter.
    pub fn keeps(&self, decision: EpisodeDecision) -> bool {
        match decision {
            EpisodeDecision::Included => true,
            EpisodeDecision::Excluded => false,
            EpisodeDecision::Unknown => self.unknown == UnknownPolicy::Keep,
        }
    }
    /// One slot per sampling interval, including the final partial interval.
    /// None means missing evidence. Unknown is never turned into a bright score.
    pub fn decide(&self, scores: &[Option<f32>]) -> EpisodeDecision {
        if self.validate().is_err()
            || scores.is_empty()
            || scores.iter().any(|score| {
                score.is_none_or(|score| !score.is_finite() || !(0.0..=1.0).contains(&score))
            })
        {
            return EpisodeDecision::Unknown;
        }
        let matching = scores
            .iter()
            .filter(|score| {
                score.is_some_and(|score| score > f32::from(self.score_above_percent) / 100.0)
            })
            .count();
        let excluded = match self.exclude_when {
            EpisodeAggregation::Any => matching > 0,
            EpisodeAggregation::MoreThanPercent { percent } => {
                (matching as u128) * 100 > (scores.len() as u128) * u128::from(percent)
            }
        };
        if excluded {
            EpisodeDecision::Excluded
        } else {
            EpisodeDecision::Included
        }
    }
}

impl WorkflowSpec {
    pub fn dataset(&self) -> Option<&DatasetSummary> {
        self.nodes().iter().find_map(|node| match node.config() {
            WorkflowNodeConfig::Dataset { source } => Some(source),
            _ => None,
        })
    }
    pub fn camera_scoring(&self) -> Option<&CameraScoring> {
        self.nodes().iter().find_map(|node| match node.config() {
            WorkflowNodeConfig::CameraScore { scoring } => Some(scoring),
            _ => None,
        })
    }
    pub fn episode_rule(&self) -> Option<&EpisodeFilterRule> {
        self.nodes().iter().find_map(|node| match node.config() {
            WorkflowNodeConfig::EpisodeFilter { rule } => Some(rule),
            _ => None,
        })
    }
    pub fn uses_native_episodes(&self) -> bool {
        self.nodes().iter().any(|node| {
            matches!(
                node.config(),
                WorkflowNodeConfig::Dataset { .. }
                    | WorkflowNodeConfig::CameraScore { .. }
                    | WorkflowNodeConfig::EpisodeFilter { .. }
            )
        })
    }
    pub fn select_dataset(&mut self, source: DatasetSummary) {
        if self.dataset() == Some(&source) {
            return;
        }
        let id = self
            .nodes()
            .iter()
            .find(|node| matches!(node.config(), WorkflowNodeConfig::Dataset { .. }))
            .map(|node| node.id().to_owned())
            .unwrap_or_else(|| "dataset".into());
        self.nodes_mut().clear();
        self.edges_mut().clear();
        self.nodes_mut().push(WorkflowNodeSpec::new(
            id,
            source.selection().label(),
            WorkflowNodeConfig::Dataset { source },
        ));
    }
    pub fn set_episode_filter(
        &mut self,
        scoring: CameraScoring,
        rule: EpisodeFilterRule,
    ) -> Result<(), String> {
        let source = self
            .nodes()
            .iter()
            .find(|node| matches!(node.config(), WorkflowNodeConfig::Dataset { .. }))
            .cloned()
            .ok_or("Choose a dataset first.")?;
        let id_for = |kind: &str| {
            self.nodes()
                .iter()
                .find(|node| {
                    matches!(
                        (kind, node.config()),
                        ("score", WorkflowNodeConfig::CameraScore { .. })
                            | ("filter", WorkflowNodeConfig::EpisodeFilter { .. })
                            | ("result", WorkflowNodeConfig::Result)
                    )
                })
                .map(|node| node.id().to_owned())
                .unwrap_or_else(|| kind.into())
        };
        let score = WorkflowNodeSpec::new(
            id_for("score"),
            "Camera scoring",
            WorkflowNodeConfig::CameraScore { scoring },
        );
        let filter = WorkflowNodeSpec::new(
            id_for("filter"),
            "Episode filter",
            WorkflowNodeConfig::EpisodeFilter { rule },
        );
        let result = WorkflowNodeSpec::new(
            id_for("result"),
            "Selected episodes",
            WorkflowNodeConfig::Result,
        );
        let nodes = vec![source, score, filter, result];
        let edges = nodes
            .windows(2)
            .map(|pair| WorkflowEdgeSpec::new(pair[0].id(), pair[1].id()))
            .collect();
        *self.nodes_mut() = nodes;
        *self.edges_mut() = edges;
        Ok(())
    }
    pub fn episode_explanation(&self) -> Option<String> {
        Some(format!(
            "Dataset: {}. {} {}",
            self.dataset()?.selection().label(),
            self.camera_scoring()?.description(),
            self.episode_rule()?.description()
        ))
    }
    pub fn episode_required_columns(&self) -> Vec<String> {
        match self.camera_scoring() {
            Some(scoring) => vec![
                "id (episode identity)".into(),
                "log_time (sample timing)".into(),
                format!(
                    "{}: {} + value.format",
                    scoring.camera.topic,
                    scoring.camera.field.join(".")
                ),
                "Derived: frame score, sample coverage, episode decision".into(),
            ],
            None => vec![],
        }
    }
    pub(super) fn validate_episode_graph(&self) -> WorkflowValidationReport {
        let mut report = WorkflowValidationReport::default();
        let result = self.check_episode_graph();
        if let Err(message) = result {
            report.errors.push(WorkflowValidationIssue::new(
                "episode_filter",
                &message,
                "Inspect the dataset, then ask the Assistant to refine this filter.",
            ));
        }
        report
    }
    fn check_episode_graph(&self) -> Result<(), String> {
        use crate::sql::workflow::WorkflowNodeId;
        WorkflowNodeId::try_from(self.id()).map_err(|error| error.to_string())?;
        if self.name().trim().is_empty() {
            return Err("Name this workflow.".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for node in self.nodes() {
            WorkflowNodeId::try_from(node.id()).map_err(|error| error.to_string())?;
            if !ids.insert(node.id()) {
                return Err("Workflow node identities must be unique.".into());
            }
        }
        let source = self
            .dataset()
            .ok_or("The filter needs one dataset source.")?;
        DatasetSelection::new(source.selection().path().to_vec())?;
        if !source.selection().can_preview()
            || source.resource_id().is_empty()
            || source.version_id().is_empty()
        {
            return Err("Choose a native dataset from the catalog.".into());
        }
        // A selected source is a valid authoring draft, but cannot execute.
        if self.nodes().len() == 1 && self.edges().is_empty() {
            return Ok(());
        }
        if self.nodes().len() != 4 || self.edges().len() != 3 {
            return Err("Use Dataset → Camera scoring → Episode filter → Result.".into());
        }
        let order = self.topological_order()?;
        let configs = order
            .iter()
            .map(|id| {
                self.nodes()
                    .iter()
                    .find(|node| node.id() == id)
                    .unwrap()
                    .config()
            })
            .collect::<Vec<_>>();
        if !matches!(
            configs.as_slice(),
            [
                WorkflowNodeConfig::Dataset { .. },
                WorkflowNodeConfig::CameraScore { .. },
                WorkflowNodeConfig::EpisodeFilter { .. },
                WorkflowNodeConfig::Result
            ]
        ) {
            return Err("Use Dataset → Camera scoring → Episode filter → Result.".into());
        }
        for pair in order.windows(2) {
            if self
                .edges()
                .iter()
                .filter(|edge| edge.source() == pair[0] && edge.target() == pair[1])
                .count()
                != 1
            {
                return Err("Connect the episode-filter nodes in sequence.".into());
            }
        }
        let scoring = self
            .camera_scoring()
            .ok_or("Choose a camera scoring rule.")?;
        scoring.validate()?;
        if scoring.camera.revision.version() != source.version_id() {
            return Err("The camera binding belongs to a different dataset version. Inspect the dataset again.".into());
        }
        self.episode_rule()
            .ok_or("Choose an episode rule.")?
            .validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DatasetSummary {
        DatasetSummary::new(
            DatasetSelection::new(vec!["dataset_demo".into()]).unwrap(),
            ("dataset-resource".into(), "version-one".into()),
        )
    }
    fn scoring(topic: &str) -> CameraScoring {
        CameraScoring::new(
            CameraBinding::new(
                topic.into(),
                TopicRevision::new("version-one".into(), "video-schema".into()),
            ),
            "a dimly lit scene".into(),
        )
    }
    fn rule() -> EpisodeFilterRule {
        EpisodeFilterRule::new(50, EpisodeAggregation::MoreThanPercent { percent: 50 })
    }
    #[test]
    fn episode_filter_corrections_and_reopen_preserve_identity_and_meaning() {
        let mut graph = WorkflowSpec::new("workflow-one", "Lighting");
        graph.select_dataset(source());
        assert!(graph.validate().valid());
        graph.set_episode_filter(scoring("/top"), rule()).unwrap();
        assert!(graph.validate().valid());
        let node_ids = graph
            .nodes()
            .iter()
            .map(|node| node.id().to_owned())
            .collect::<Vec<_>>();
        graph
            .set_episode_filter(
                scoring("/wrist"),
                EpisodeFilterRule::new(50, EpisodeAggregation::Any),
            )
            .unwrap();
        assert_eq!(graph.id(), "workflow-one");
        assert_eq!(
            graph
                .nodes()
                .iter()
                .map(|node| node.id())
                .collect::<Vec<_>>(),
            node_ids
        );
        let reopened: WorkflowSpec =
            serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        assert_eq!(reopened, graph);
        let explanation = reopened.episode_explanation().unwrap();
        assert!(explanation.contains("/wrist"));
        assert!(!explanation.contains("/top"));
        assert!(explanation.contains("any sampled frame"));
        assert!(
            reopened
                .episode_required_columns()
                .iter()
                .any(|field| field.contains("/wrist"))
        );
    }
    #[test]
    fn missing_evidence_and_boundary_scores_have_explicit_decisions() {
        let rule = rule();
        assert!(!rule.keeps(EpisodeDecision::Unknown));
        let mut keep_unknown = rule.clone();
        keep_unknown.unknown = UnknownPolicy::Keep;
        assert!(keep_unknown.keeps(keep_unknown.decide(&[None])));
        assert!(!keep_unknown.keeps(EpisodeDecision::Excluded));
        assert!(keep_unknown.keeps(EpisodeDecision::Included));
        assert_eq!(rule.decide(&[]), EpisodeDecision::Unknown);
        assert_eq!(rule.decide(&[None]), EpisodeDecision::Unknown);
        assert_eq!(rule.decide(&[Some(0.9), None]), EpisodeDecision::Unknown);
        assert_eq!(rule.decide(&[Some(f32::NAN)]), EpisodeDecision::Unknown);
        assert_eq!(rule.decide(&[Some(1.1)]), EpisodeDecision::Unknown);
        assert_eq!(rule.decide(&[Some(0.5)]), EpisodeDecision::Included);
        assert_eq!(
            rule.decide(&[Some(0.9), Some(0.1)]),
            EpisodeDecision::Included
        );
        assert_eq!(
            rule.decide(&[Some(0.9), Some(0.9), Some(0.1)]),
            EpisodeDecision::Excluded
        );
        assert_eq!(
            EpisodeFilterRule::new(50, EpisodeAggregation::Any).decide(&[Some(0.9), Some(0.1)]),
            EpisodeDecision::Excluded
        );
    }
    #[test]
    fn native_drafts_reject_invalid_sampling_bindings_and_graph_structure() {
        let mut graph = WorkflowSpec::new("workflow-one", "Lighting");
        graph.select_dataset(source());
        graph.set_episode_filter(scoring("/top"), rule()).unwrap();
        for mutate in ["interval", "field", "version", "threshold", "edge"] {
            let mut malformed = serde_json::to_value(&graph).unwrap();
            match mutate {
                "interval" => {
                    malformed["nodes"][1]["scoring"]["interval_ms"] = serde_json::json!(0)
                }
                "field" => {
                    malformed["nodes"][1]["scoring"]["camera"]["field"] =
                        serde_json::json!(["invented"])
                }
                "version" => {
                    malformed["nodes"][1]["scoring"]["camera"]["revision"]["version"] =
                        serde_json::json!("different-version")
                }
                "threshold" => {
                    malformed["nodes"][2]["rule"]["score_above_percent"] = serde_json::json!(101)
                }
                "edge" => malformed["edges"][2]["target"] = serde_json::json!("dataset"),
                _ => unreachable!(),
            }
            let malformed: WorkflowSpec = serde_json::from_value(malformed).unwrap();
            assert!(!malformed.validate().valid(), "{mutate}");
        }
        let mut unsupported = serde_json::to_value(&graph).unwrap();
        unsupported["nodes"][1]["scoring"]["producer"] = serde_json::json!("invented-model");
        assert!(serde_json::from_value::<WorkflowSpec>(unsupported).is_err());
        graph.select_dataset(DatasetSummary::new(
            DatasetSelection::new(vec!["another_dataset".into()]).unwrap(),
            ("another-resource".into(), "another-version".into()),
        ));
        assert_eq!(graph.nodes().len(), 1);
        assert!(graph.camera_scoring().is_none());
        assert!(graph.episode_explanation().is_none());
        assert!(graph.validate().valid());
    }
}
