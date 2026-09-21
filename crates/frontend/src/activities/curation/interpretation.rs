//! Browser-owned semantic binding over the backend's credentialed Decisions transport.
#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests;

use crate::client::model::ModelClient;
use serde_json::{Value, json};
use spiral_robotics_common::curation::{
    interpretation::{InterpretationRequest, RuleIntent, RuleProposal},
    rules::ConditionOp,
};
use spiral_robotics_common::workflow::document::{DecisionRequest, DecisionResponse};
use std::collections::BTreeMap;

pub(super) struct RuleInterpreter;
impl RuleInterpreter {
    pub(super) async fn interpret(
        model: ModelClient,
        request: InterpretationRequest,
    ) -> Result<RuleProposal, String> {
        request.validate()?;
        let quantities = QuantityCandidate::extract(request.prompt());
        let answers = model
            .decide(&DecisionRequest::new(
                "typesafe/jev-1.13",
                json!({"request": request.prompt(), "quantities": quantities}),
                Self::questions(&request, &quantities),
            ))
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self::bind(&request, &quantities, answers))
    }
    fn choice(instructions: &str, criteria: BTreeMap<String, String>) -> Value {
        let mut criteria = criteria;
        criteria.insert(
            "unresolved".into(),
            "Not specified, ambiguous, unsupported, or none of these choices.".into(),
        );
        json!({"type": "choice", "instructions": instructions, "criteria": criteria})
    }
    fn questions(request: &InterpretationRequest, quantities: &[QuantityCandidate]) -> Value {
        let mut questions = json!({
            "operation": {"type": "choice", "instructions": "Read `request`. Which single dataset property or operation is requested? Classify informal grammar by meaning. Omitted thresholds are handled separately. Choose unresolved for multiple independent changes or no requested change.", "criteria": Self::operation_choices()},
            "comparison": Self::choice("Read `request`. For a whole-episode filter, choose the condition under which episodes must be EXCLUDED. A keep condition requires its logical complement. Respect negation. For an interval transform, multiple conditions, or no requested change choose unresolved.", [("below", "Exclude values strictly less than the threshold (<)"), ("at_most", "Exclude values less than or equal to the threshold (<=)"), ("above", "Exclude values strictly greater than the threshold (>)"), ("at_least", "Exclude values greater than or equal to the threshold (>=)")].into_iter().map(|(key, label)| (key.into(), label.into())).collect()),
            "quantity": Self::choice("Read `request` and its exact extracted `quantities`. Which single quantity is the requested whole-episode filter threshold? For Remove unproductive moments, choose unresolved; it has a built-in policy. Each candidate supplies its literal and surrounding source context. Ignore camera identifiers, frame indices and rejected examples. Select unresolved when no threshold is given, multiple thresholds are requested, or the intended quantity is ambiguous. Unit conversion is done by code, not this decision.", quantities.iter().enumerate().map(|(index, quantity)| (format!("q{index}"), serde_json::to_string(quantity).expect("finite extracted quantity"))).collect())
        });
        if !request.cameras().is_empty() {
            questions["camera"] = Self::choice(
                "Read `request`. Which exact listed camera does it identify? If no camera is specified or multiple match, choose unresolved.",
                request
                    .cameras()
                    .iter()
                    .enumerate()
                    .map(|(index, camera)| (format!("c{index}"), camera.clone()))
                    .collect(),
            );
        }
        questions
    }
    /// Semantic choices are independent of the short labels displayed in Studio.
    fn operation_choices() -> BTreeMap<String, String> {
        [
            (RuleIntent::Duration, "Keep or remove whole episodes based on their recording duration or length in time."),
            (RuleIntent::MessageCount, "The instruction refers to messages or the number of recorded messages in an episode, even if the word count is omitted."),
            (RuleIntent::CameraLuminance, "Keep or remove whole episodes based on a camera image being too dark, too bright, or having a specified luminance."),
            (RuleIntent::ProductiveChunks, "Remove unproductive or low-progress moments within episodes, retaining the productive time intervals for training."),
            (RuleIntent::Unresolved, "The instruction refers to none of these properties or operations, or requests multiple independent changes. Do not select this merely for informal grammar or a missing comparison or threshold."),
        ]
        .into_iter()
        .map(|(intent, description)| (intent.key().to_owned(), description.to_owned()))
        .collect()
    }
    fn bind(
        request: &InterpretationRequest,
        quantities: &[QuantityCandidate],
        response: DecisionResponse,
    ) -> RuleProposal {
        let mut proposal = RuleProposal::default();
        let intent = response
            .choice("operation")
            .map(RuleIntent::from_key)
            .unwrap_or(RuleIntent::Unresolved);
        proposal.set_intent(intent);
        proposal.set_comparison(
            response
                .choice("comparison")
                .and_then(ConditionOp::from_key)
                .filter(|op| {
                    matches!(
                        op,
                        ConditionOp::Below
                            | ConditionOp::AtMost
                            | ConditionOp::Above
                            | ConditionOp::AtLeast
                    )
                }),
        );
        let quantity = response
            .choice("quantity")
            .and_then(|key| key.strip_prefix('q'))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| quantities.get(index));
        let value = if intent == RuleIntent::ProductiveChunks {
            None
        } else if intent == RuleIntent::Duration {
            let seconds = quantity.and_then(QuantityCandidate::duration_seconds);
            if seconds.is_some() {
                proposal.set_unit(Some("seconds".into()));
            }
            seconds
        } else {
            quantity.map(QuantityCandidate::value)
        };
        proposal.set_value(value);
        let camera = response
            .choice("camera")
            .and_then(|key| key.strip_prefix('c'))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| request.cameras().get(index))
            .cloned();
        proposal.set_camera(camera);
        if intent == RuleIntent::CameraLuminance {
            proposal.add_note(
                "Measures linear relative luminance from frame 0 of the selected camera.",
            );
        }
        proposal
    }
}
/// Exact quantities with source evidence; semantic binding selects, never invents, one.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
struct QuantityCandidate {
    text: String,
    value: f64,
    duration_scale: Option<f64>,
    span: std::ops::Range<usize>,
    context: String,
}
impl QuantityCandidate {
    const fn value(&self) -> f64 {
        self.value
    }
    /// Normalize only an explicit unit attached to this exact numeric literal.
    fn duration_seconds(&self) -> Option<f64> {
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
    fn extract(text: &str) -> Vec<Self> {
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

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
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
            assert_eq!(candidates[0].text, literal);
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
            assert_eq!(&prompt[candidate.span.clone()], candidate.text.as_str());
            assert!(candidate.context.contains(candidate.text.as_str()));
        }
        assert_eq!(candidates[0].duration_seconds(), None);
        assert_eq!(candidates[1].duration_seconds(), Some(20.0));
        assert_eq!(candidates[2].duration_seconds(), Some(30.0));
        assert_eq!(candidates[3].value(), 4000.0);
        let punctuated = QuantityCandidate::extract("4000, then 20s");
        assert_eq!(punctuated[0].text, "4000");
        assert_eq!(punctuated[0].span, 0..4);
        let repeated = QuantityCandidate::extract("camera 20: under 20s");
        assert_ne!(repeated[0].span, repeated[1].span);
    }

    #[test]
    fn semantic_quantity_answers_cannot_configure_progress_policy() {
        let request = InterpretationRequest::new(
            "remove low progress training episode parts below 0.5".into(),
            Vec::new(),
        );
        let response = serde_json::from_value(json!({"answers": {
            "operation": {"type":"choice","choice":"productive_chunks","confidence":1.0},
            "quantity": {"type":"choice","choice":"q0","confidence":1.0}
        }}))
        .unwrap();
        let proposal = RuleInterpreter::bind(
            &request,
            &QuantityCandidate::extract(request.prompt()),
            response,
        );
        assert_eq!(proposal.value(), None);
        assert!(proposal.problems().is_empty());
    }
    #[test]
    fn duration_units_belong_to_selected_quantity_and_are_never_borrowed() {
        let request = InterpretationRequest::new(
            "camera 2: remove episodes shorter than 20; example 30s".into(),
            Vec::new(),
        );
        let quantities = QuantityCandidate::extract(request.prompt());
        let answers = serde_json::from_value(json!({"answers": {
            "operation": {"type":"choice","choice":"duration","confidence":1.0},
            "comparison": {"type":"choice","choice":"below","confidence":1.0},
            "quantity": {"type":"choice","choice":"q1","confidence":1.0},
            "unit": {"type":"choice","choice":"seconds","confidence":1.0}
        }}))
        .unwrap();
        let proposal = RuleInterpreter::bind(&request, &quantities, answers);
        assert_eq!(proposal.value(), None);
        assert!(!proposal.problems().is_empty());
        let questions = RuleInterpreter::questions(&request, &quantities);
        assert!(questions.get("unit").is_none());
        assert!(questions.get("camera").is_none());
        assert!(
            questions["quantity"]["criteria"]["q1"]
                .as_str()
                .unwrap()
                .contains("example 30s")
        );
    }
    #[test]
    fn semantic_binding_cannot_invent_a_number_or_camera() {
        let request = InterpretationRequest::new(
            "Exclude episodes below 2 minutes".into(),
            vec!["/top".into()],
        );
        let answers = serde_json::from_value(json!({"answers": {
            "operation": {"type":"choice","choice":"duration","confidence":1.0},
            "comparison": {"type":"choice","choice":"below","confidence":1.0},
            "quantity": {"type":"choice","choice":"q99","confidence":1.0},
            "camera": {"type":"choice","choice":"/invented","confidence":1.0},
            "unit": {"type":"choice","choice":"minutes","confidence":1.0}
        }}))
        .unwrap();
        let proposal = RuleInterpreter::bind(
            &request,
            &QuantityCandidate::extract(request.prompt()),
            answers,
        );
        assert_eq!(proposal.value(), None);
        assert_eq!(proposal.camera(), None);
        assert!(!proposal.problems().is_empty());
    }
    #[test]
    fn selected_duration_literals_normalize_without_model_unit_guesses() {
        for (literal, expected) in [
            ("20s", 20.0),
            ("20 seconds", 20.0),
            ("500ms", 0.5),
            ("2min", 120.0),
        ] {
            let request = InterpretationRequest::new(
                format!("remove duration shorter than {literal}"),
                Vec::new(),
            );
            let answers = serde_json::from_value(json!({"answers": {
                "operation": {"type":"choice","choice":"duration","confidence":1.0},
                "comparison": {"type":"choice","choice":"below","confidence":1.0},
                "quantity": {"type":"choice","choice":"q0","confidence":1.0},
                "unit": {"type":"choice","choice":"unresolved","confidence":1.0}
            }}))
            .unwrap();
            let proposal = RuleInterpreter::bind(
                &request,
                &QuantityCandidate::extract(request.prompt()),
                answers,
            );
            assert_eq!(proposal.value(), Some(expected), "{literal}");
            assert!(
                proposal.problems().is_empty(),
                "{literal}: {:?}",
                proposal.problems()
            );
        }
    }
    #[test]
    fn explicit_units_never_bypass_an_unresolved_number() {
        let request =
            InterpretationRequest::new("remove duration shorter than 20s".into(), Vec::new());
        for (choice, confidence) in [("q0", 0.17), ("q99", 1.0), ("unresolved", 1.0)] {
            let answers = serde_json::from_value(json!({"answers": {
                "operation": {"type":"choice","choice":"duration","confidence":1.0},
                "comparison": {"type":"choice","choice":"below","confidence":1.0},
                "quantity": {"type":"choice","choice":choice,"confidence":confidence},
                "unit": {"type":"choice","choice":"seconds","confidence":1.0}
            }}))
            .unwrap();
            let proposal = RuleInterpreter::bind(
                &request,
                &QuantityCandidate::extract(request.prompt()),
                answers,
            );
            assert_eq!(proposal.value(), None);
            assert!(!proposal.problems().is_empty());
        }
    }
}
