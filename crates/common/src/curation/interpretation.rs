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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
    pub fn progress(&self) -> Option<&super::progress::ProgressSelection> {
        self.progress.as_ref()
    }
    pub fn set_progress(&mut self, progress: super::progress::ProgressSelection) {
        self.progress = Some(progress);
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
        match self.intent {
            RuleIntent::Unresolved => {
                problems.push("Choose the operation this rule should perform.".into())
            }
            _ => {}
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

/// Exact quantities with source evidence; semantic binding selects, never invents, one.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct QuantityCandidate {
    text: String,
    value: f64,
    duration_scale: Option<f64>,
    span: std::ops::Range<usize>,
    context: String,
}
impl QuantityCandidate {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub const fn value(&self) -> f64 {
        self.value
    }
    /// Normalize only an explicit unit attached to this exact numeric literal.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration_scale.map(|scale| self.value * scale)
    }
    fn duration_suffix(suffix: &str) -> Option<(f64, usize)> {
        let trimmed = suffix.trim_start();
        let letters = trimmed.bytes().take_while(u8::is_ascii_alphabetic).count();
        let scale = match trimmed[..letters].to_ascii_lowercase().as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "ms" | "msec" | "msecs" | "millisecond" | "milliseconds" => 0.001,
            "min" | "mins" | "minute" | "minutes" => 60.0,
            _ => return None,
        };
        Some((scale, suffix.len() - trimmed.len() + letters))
    }
    fn local_context(text: &str, span: &std::ops::Range<usize>) -> String {
        let start = text[..span.start]
            .char_indices()
            .rev()
            .nth(48)
            .map_or(0, |(index, _)| index);
        let end = text[span.end..]
            .char_indices()
            .nth(48)
            .map_or(text.len(), |(index, _)| span.end + index);
        text[start..end].to_owned()
    }
    pub fn extract(text: &str) -> Vec<Self> {
        // Keep exponent and grouped-thousands notation together. A malformed
        // literal is unresolved rather than silently split into other numbers.
        let bytes = text.as_bytes();
        let mut cursor = 0;
        let mut candidates = Vec::new();
        while cursor < bytes.len() && candidates.len() < 32 {
            if !bytes[cursor].is_ascii_digit() && !matches!(bytes[cursor], b'.' | b'-' | b'+') {
                cursor += 1;
                continue;
            }
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_digit()
                    || matches!(bytes[cursor], b'.' | b'-' | b'+' | b'e' | b'E' | b','))
            {
                cursor += 1;
            }
            let token = text[start..cursor].trim_end_matches(',');
            let normalized = token.replace(',', "");
            let valid_groups = !token.contains(',')
                || token.split(',').enumerate().all(|(index, group)| {
                    let digits = group.trim_start_matches(['-', '+']);
                    (if index == 0 {
                        (1..=3).contains(&digits.len())
                    } else {
                        digits.len() == 3
                    }) && digits.bytes().all(|byte| byte.is_ascii_digit())
                });
            if valid_groups
                && let Ok(value) = normalized.parse::<f64>()
                && value.is_finite()
            {
                let mut end = start + token.len();
                let duration_scale = Self::duration_suffix(&text[end..]).map(|(scale, length)| {
                    end += length;
                    cursor = end;
                    scale
                });
                candidates.push(Self {
                    text: if duration_scale.is_some() {
                        text[start..end].to_owned()
                    } else {
                        token.to_owned()
                    },
                    value,
                    duration_scale,
                    span: start..end,
                    context: Self::local_context(text, &(start..end)),
                });
            }
        }
        candidates
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
    fn numeric_candidates_preserve_negative_and_fractional_values() {
        let candidates = QuantityCandidate::extract("remove under 2.5 seconds, not 30; -1");
        assert_eq!(
            candidates
                .iter()
                .map(QuantityCandidate::value)
                .collect::<Vec<_>>(),
            [2.5, 30.0, -1.0]
        );
    }
    #[test]
    fn scientific_and_grouped_numbers_are_not_split_into_wrong_thresholds() {
        let values =
            QuantityCandidate::extract("1e3 messages or 2,500 messages; -0.5 EV, malformed 2,50")
                .iter()
                .map(QuantityCandidate::value)
                .collect::<Vec<_>>();
        assert_eq!(values, [1000.0, 2500.0, -0.5]);
    }
    #[test]
    fn duration_candidates_preserve_compact_and_spaced_units() {
        for (literal, seconds) in [
            ("20s", 20.0),
            ("20 seconds", 20.0),
            ("500ms", 0.5),
            ("500 milliseconds", 0.5),
            ("2min", 120.0),
            ("1.5 minutes", 90.0),
            ("2e1s", 20.0),
        ] {
            let candidates = QuantityCandidate::extract(&format!("under {literal}"));
            assert_eq!(candidates.len(), 1, "{literal}");
            assert_eq!(candidates[0].text(), literal);
            assert_eq!(candidates[0].duration_seconds(), Some(seconds));
        }
        for literal in ["20", "20 messages", "20samples", "20meters"] {
            let candidates = QuantityCandidate::extract(literal);
            assert_eq!(candidates.len(), 1, "{literal}");
            assert_eq!(candidates[0].duration_seconds(), None, "{literal}");
        }
    }
    #[test]
    fn quantity_evidence_preserves_spans_context_and_distinct_occurrences() {
        let prompt = "相机 2: exclude below 20s, not 30s; messages over 4,000";
        let candidates = QuantityCandidate::extract(prompt);
        assert_eq!(candidates.len(), 4);
        for candidate in &candidates {
            assert_eq!(&prompt[candidate.span.clone()], candidate.text());
            assert!(candidate.context.contains(candidate.text()));
        }
        assert_eq!(candidates[0].duration_seconds(), None);
        assert_eq!(candidates[1].duration_seconds(), Some(20.0));
        assert_eq!(candidates[2].duration_seconds(), Some(30.0));
        assert_eq!(candidates[3].value(), 4000.0);
        let punctuated = QuantityCandidate::extract("4000, then 20s");
        assert_eq!(punctuated[0].text(), "4000");
        assert_eq!(punctuated[0].span, 0..4);
        let repeated = QuantityCandidate::extract("camera 20: under 20s");
        assert_ne!(repeated[0].span, repeated[1].span);
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
    #[test]
    fn message_count_proposal_uses_the_builtin_fact_key() {
        let mut proposal = RuleProposal::default();
        proposal.set_intent(RuleIntent::MessageCount);
        proposal.set_comparison(Some(ConditionOp::Below));
        proposal.set_value(Some(3000.0));
        let mut spec = CurationSpec::new("test", "Test").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_test".into()]).unwrap(),
        );
        proposal.apply(&mut spec, "messages").unwrap();
        assert!(
            spec.compile()
                .unwrap()
                .roster_sql()
                .contains("message_count < 3000")
        );
    }
    #[test]
    fn filters_from_either_authoring_path_compile_identically() {
        let mut proposal = RuleProposal::default();
        proposal.set_intent(RuleIntent::Duration);
        proposal.set_comparison(Some(ConditionOp::Below));
        proposal.set_value(Some(5.0));
        proposal.set_unit(Some("seconds".into()));
        let mut spec = CurationSpec::new("test", "Test").with_source(
            crate::datasets::DatasetSelection::new(vec!["dataset_test".into()]).unwrap(),
        );
        proposal.apply(&mut spec, "short").unwrap();
        assert!(
            spec.compile()
                .unwrap()
                .roster_sql()
                .contains("duration_ns / 1000000000.0 < 5")
        );
    }
}
