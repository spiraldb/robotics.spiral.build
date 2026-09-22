//! Editable decisions from natural-language authoring. Numerical values are
//! extracted by code; a model may only select a supplied candidate.
use super::{
    CurationSpec,
    facts::{CameraSample, CurationFact, FactMeasure, TopicRef},
    rules::{
        ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition,
        TransformOperation,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InterpretationRequest {
    prompt: String,
    cameras: Vec<String>,
}
impl InterpretationRequest {
    pub fn new(prompt: String, cameras: Vec<String>) -> Self {
        Self { prompt, cameras }
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn cameras(&self) -> &[String] {
        &self.cameras
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.prompt.trim().is_empty() || self.prompt.len() > 4000 {
            return Err("Describe one rule in 1–4000 characters.".into());
        }
        if self.cameras.len() > 64 || self.cameras.iter().any(|camera| camera.len() > 512) {
            return Err("The camera list is too large.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleIntent {
    Duration,
    MessageCount,
    CameraLuminance,
    ProductiveChunks,
    Unresolved,
}
impl RuleIntent {
    pub const ALL: [Self; 5] = [
        Self::Duration,
        Self::MessageCount,
        Self::CameraLuminance,
        Self::ProductiveChunks,
        Self::Unresolved,
    ];
    pub const fn key(self) -> &'static str {
        match self {
            Self::Duration => "duration",
            Self::MessageCount => "message_count",
            Self::CameraLuminance => "camera_luminance",
            Self::ProductiveChunks => "productive_chunks",
            Self::Unresolved => "unresolved",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Duration => "Filter · episode duration",
            Self::MessageCount => "Filter · message count",
            Self::CameraLuminance => "Filter · camera luminance",
            Self::ProductiveChunks => "Transform · Remove unproductive moments",
            Self::Unresolved => "Choose a rule",
        }
    }
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|intent| intent.key() == key)
            .unwrap_or(Self::Unresolved)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleProposal {
    intent: RuleIntent,
    comparison: Option<ConditionOp>,
    value: Option<f64>,
    camera: Option<String>,
    unit: Option<String>,
    notes: Vec<String>,
    #[serde(default)]
    progress: Option<super::progress::ProgressSelection>,
}
impl Default for RuleProposal {
    fn default() -> Self {
        Self {
            intent: RuleIntent::Unresolved,
            comparison: None,
            value: None,
            camera: None,
            unit: None,
            notes: Vec::new(),
            progress: None,
        }
    }
}
impl RuleProposal {
    pub const fn intent(&self) -> RuleIntent {
        self.intent
    }
    pub const fn comparison(&self) -> Option<ConditionOp> {
        self.comparison
    }
    pub const fn value(&self) -> Option<f64> {
        self.value
    }
    pub fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
    pub fn set_intent(&mut self, intent: RuleIntent) {
        if intent == RuleIntent::ProductiveChunks && self.progress.is_none() {
            self.progress = Some(super::progress::ProgressSelection::new(String::new()));
        }
        self.intent = intent;
    }
    pub fn set_comparison(&mut self, comparison: Option<ConditionOp>) {
        self.comparison = comparison;
    }
    pub fn set_value(&mut self, value: Option<f64>) {
        self.value = value;
    }
    pub fn set_camera(&mut self, camera: Option<String>) {
        self.camera = camera.filter(|camera| !camera.is_empty());
    }
    pub fn set_unit(&mut self, unit: Option<String>) {
        self.unit = unit;
    }
    pub fn add_note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.intent == RuleIntent::Unresolved {
            problems.push("Choose the operation this rule should perform.".into());
        }
        if !matches!(self.intent, RuleIntent::ProductiveChunks)
            && !matches!(
                self.comparison,
                Some(
                    ConditionOp::Below
                        | ConditionOp::AtMost
                        | ConditionOp::Above
                        | ConditionOp::AtLeast
                )
            )
        {
            problems.push("Choose when to exclude an episode.".into());
        }
        if self.intent != RuleIntent::ProductiveChunks
            && self
                .value
                .is_none_or(|value| !value.is_finite() || value < 0.0)
        {
            problems.push("Enter a finite, non-negative threshold.".into());
        }
        if self.intent == RuleIntent::MessageCount
            && self.value.is_some_and(|value| value.fract() != 0.0)
        {
            problems.push("Message count must be a whole number.".into());
        }
        if matches!(self.intent, RuleIntent::CameraLuminance) {
            if self
                .camera
                .as_deref()
                .is_none_or(|camera| camera.trim().is_empty())
            {
                problems.push("Choose the camera to measure.".into());
            }
            if self.intent == RuleIntent::CameraLuminance
                && self.value.is_some_and(|value| value > 1.0)
            {
                problems.push("Relative luminance must be between 0 and 1.".into());
            }
        }
        if self.intent == RuleIntent::Duration && self.unit.as_deref() != Some("seconds") {
            problems.push("Confirm the duration threshold in seconds.".into());
        }
        problems
    }
    pub fn apply(&self, spec: &mut CurationSpec, identity: &str) -> Result<(), String> {
        if let Some(problem) = self.problems().first() {
            return Err(problem.clone());
        }
        if self.intent == RuleIntent::ProductiveChunks {
            let mut selection = self
                .progress
                .clone()
                .unwrap_or_else(|| super::progress::ProgressSelection::for_dataset(spec.source()));
            selection.resolve_dataset(spec.source());
            if selection.sources_relation().is_empty() {
                return Err(match spec.source() {
                    Some(dataset) => format!("Remove unproductive moments is not available for {}. The current detector supports dataset_warp_sim_bottles_sample and its original recordings. Choose that dataset to use this transformation.", dataset.label()),
                    None => "Choose dataset_warp_sim_bottles_sample before adding Remove unproductive moments.".into(),
                });
            }
            selection.validate()?;
            spec.add_transform(CurationTransform::new(
                identity,
                "",
                TransformOperation::SelectProgressChunks { selection },
            ));
            return Ok(());
        }
        let fact = match self.intent {
            RuleIntent::Duration => "duration_s".to_owned(),
            RuleIntent::MessageCount => "messages".to_owned(),
            RuleIntent::CameraLuminance => {
                let key = format!("luminance_{identity}");
                let topic = self.camera.as_deref().ok_or("Choose a camera.")?;
                spec.add_fact(CurationFact::new(
                    &key,
                    format!("{topic} luminance"),
                    FactMeasure::CameraBrightness {
                        camera: CameraSample::new(TopicRef::new(topic)),
                    },
                ));
                key
            }
            _ => return Err("This operation is not executable yet.".into()),
        };
        spec.add_rule(CurationRule::new(
            identity,
            "",
            RuleCondition::new(
                fact,
                self.comparison.ok_or("Choose a comparison.")?,
                ConditionValue::Number(self.value.ok_or("Enter a threshold.")?),
            ),
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unresolved_values_never_become_executable_defaults() {
        let mut proposal = RuleProposal::default();
        proposal.set_intent(RuleIntent::Duration);
        let mut spec = CurationSpec::new("test", "Test");
        assert!(proposal.apply(&mut spec, "rule").is_err());
        assert!(spec.rules().is_empty());
    }
    #[test]
    fn warp_defaults_resolve_only_for_the_matching_dataset() {
        let mut proposal = RuleProposal::default();
        proposal.set_intent(RuleIntent::ProductiveChunks);
        assert!(proposal.problems().is_empty());
        let mut supported = CurationSpec::new("test", "Test").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_warp_sim_bottles_sample".into()])
                .unwrap(),
        );
        proposal.apply(&mut supported, "warp").unwrap();
        let TransformOperation::SelectProgressChunks { selection } =
            supported.transforms()[0].operation()
        else {
            panic!("expected WARP");
        };
        assert_eq!(selection.sources_relation(), "warp_sim_bottles_sources_v1");
        assert_eq!(selection.minimum_velocity(), 0.0);
        assert_eq!(selection.chunk_frames(), 30);
        let mut unsupported = CurationSpec::new("other", "Other").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_other".into()]).unwrap(),
        );
        assert!(
            proposal
                .apply(&mut unsupported, "warp")
                .unwrap_err()
                .contains("dataset_other")
        );
        assert!(unsupported.transforms().is_empty());
    }
    #[test]
    fn automatic_progress_ignores_legacy_proposal_cutoffs_and_rejects_abc130k() {
        let mut proposal = RuleProposal::default();
        proposal.set_intent(RuleIntent::ProductiveChunks);
        proposal.set_value(Some(0.5));
        let mut supported = CurationSpec::new("test", "Test").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_warp_sim_bottles_sample".into()])
                .unwrap(),
        );
        proposal.apply(&mut supported, "automatic").unwrap();
        let TransformOperation::SelectProgressChunks { selection } =
            supported.transforms()[0].operation()
        else {
            panic!("expected progress selection")
        };
        assert_eq!(selection.minimum_velocity(), 0.0);
        let mut unsupported = CurationSpec::new("test", "Test").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_abc130k_sample".into()]).unwrap(),
        );
        let error = proposal.apply(&mut unsupported, "automatic").unwrap_err();
        assert!(error.contains("dataset_abc130k_sample"));
        assert!(error.contains("dataset_warp_sim_bottles_sample"));
        assert!(unsupported.transforms().is_empty());
    }
}
