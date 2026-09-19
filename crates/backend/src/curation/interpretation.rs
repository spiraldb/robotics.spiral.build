//! Bounded semantic binding through OpenRouter's Decisions API.
use crate::workflow::provider::ProviderConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spiral_robotics_common::curation::{
    interpretation::{InterpretationRequest, QuantityCandidate, RuleIntent, RuleProposal},
    rules::ConditionOp,
};
use std::{collections::BTreeMap, time::Duration};

pub(super) struct RuleInterpreter;
impl RuleInterpreter {
    pub(super) async fn interpret(request: InterpretationRequest) -> Result<RuleProposal, String> {
        request.validate()?;
        let quantities = QuantityCandidate::extract(request.prompt());
        let answers = Self::decide(&request, &quantities).await?;
        Ok(Self::bind(&request, &quantities, answers))
    }
    async fn decide(
        request: &InterpretationRequest,
        quantities: &[QuantityCandidate],
    ) -> Result<DecisionResponse, String> {
        let questions = Self::questions(request, quantities);
        let config = ProviderConfig::from_env()?
            .ok_or("Configure the Assistant's OpenRouter credential to interpret a rule.")?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Could not create the rule interpretation client.")?;
        let response = config.decisions_request(&client)?.json(&json!({
            "model": "typesafe/jev-1.13", "state": {"request": request.prompt(), "quantities": quantities}, "questions": questions
        })).send().await.map_err(|_| "Rule interpretation could not reach OpenRouter. Try again or use the manual builder.")?;
        if !response.status().is_success() {
            return Err(format!(
                "OpenRouter rule interpretation returned HTTP {}. Try again or use the manual builder.",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > 128 * 1024)
        {
            return Err("The interpretation response exceeded its size limit.".into());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Could not read the interpretation response.")?;
        if bytes.len() > 128 * 1024 {
            return Err("The interpretation response exceeded its size limit.".into());
        }
        let answers: DecisionResponse = serde_json::from_slice(&bytes)
            .map_err(|_| "OpenRouter returned an invalid Decisions response.")?;
        Ok(answers)
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
#[derive(Debug, Deserialize, Serialize)]
struct DecisionResponse {
    answers: BTreeMap<String, ChoiceAnswer>,
}
impl DecisionResponse {
    fn choice(&self, question: &str) -> Option<&str> {
        let answer = self.answers.get(question)?;
        // This conservative floor is a product review policy, not a probability
        // of correctness. Retain the distribution for diagnostics/evaluation.
        tracing::debug!(question, choice = %answer.choice, confidence = ?answer.confidence, probabilities = ?answer.probabilities, "Rule interpretation decision");
        (answer.kind == "choice"
            && answer.confidence.is_some_and(|confidence| {
                confidence.is_finite() && (0.5..=1.0).contains(&confidence)
            }))
        .then_some(answer.choice.as_str())
    }
}
#[derive(Debug, Deserialize, Serialize)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    confidence: Option<f64>,
    #[serde(default)]
    probabilities: BTreeMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "bounded 16-case live evaluation; requires configured OpenRouter credential"]
    async fn live_representative_quantity_binding() {
        let cases = [
            (
                "remove duration shorter than 20s",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            (
                "Exclude recordings lasting less than 20 seconds",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            (
                "Drop episodes under 500ms",
                Some((RuleIntent::Duration, ConditionOp::Below, 0.5)),
            ),
            (
                "Remove episodes longer than 2min",
                Some((RuleIntent::Duration, ConditionOp::Above, 120.0)),
            ),
            (
                "remove episode whose messages larger than 4000",
                Some((RuleIntent::MessageCount, ConditionOp::Above, 4000.0)),
            ),
            (
                "Remove episodes with more than 4,000 messages",
                Some((RuleIntent::MessageCount, ConditionOp::Above, 4000.0)),
            ),
            (
                "Keep episodes lasting at least 20s",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            (
                "Keep episodes with fewer than 4000 messages",
                Some((RuleIntent::MessageCount, ConditionOp::AtLeast, 4000.0)),
            ),
            (
                "For camera 2, remove episodes shorter than 20s",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            (
                "Starting inspection at frame 10, remove episodes shorter than 20 seconds",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            (
                "remove duration shorter than 20s, not 30s",
                Some((RuleIntent::Duration, ConditionOp::Below, 20.0)),
            ),
            ("remove episodes shorter than 20", None),
            ("Remove short episodes", None),
            (
                "Remove episodes shorter than 20s and more than 4000 messages",
                None,
            ),
            (
                "Remove episodes shorter than 20s or 30s, I have not decided",
                None,
            ),
            ("Do not remove any episodes", None),
        ];
        let mut failures = Vec::new();
        for (prompt, expected) in cases {
            let request = InterpretationRequest::new(prompt.into(), Vec::new());
            let quantities = QuantityCandidate::extract(prompt);
            let response = match RuleInterpreter::decide(&request, &quantities).await {
                Ok(response) => response,
                Err(error) => {
                    eprintln!("{}", json!({"prompt":prompt, "transport_error":error}));
                    failures.push(prompt);
                    continue;
                }
            };
            let diagnostic = serde_json::to_value(&response).unwrap();
            let proposal = RuleInterpreter::bind(&request, &quantities, response);
            let matches = match expected {
                Some((intent, comparison, value)) => {
                    proposal.intent() == intent
                        && proposal.comparison() == Some(comparison)
                        && proposal.value() == Some(value)
                        && proposal.problems().is_empty()
                }
                None => !proposal.problems().is_empty(),
            };
            eprintln!(
                "{}",
                json!({"prompt":prompt, "passed":matches, "answers":diagnostic, "proposal":proposal})
            );
            if !matches {
                failures.push(prompt);
            }
        }
        assert!(
            failures.is_empty(),
            "Failed representative prompts: {failures:?}"
        );
    }
    #[tokio::test]
    #[ignore = "one live interpretation of the reported transformation prompt; no video inference"]
    async fn live_automatic_progress_has_no_user_cutoff() {
        let proposal = RuleInterpreter::interpret(InterpretationRequest::new(
            "remove low progress training episode parts".into(),
            Vec::new(),
        ))
        .await
        .unwrap();
        assert_eq!(proposal.intent(), RuleIntent::ProductiveChunks);
        assert_eq!(proposal.value(), None);
        assert!(proposal.problems().is_empty());
        let mut spec =
            spiral_robotics_common::curation::CurationSpec::new("review-test", "Review test")
                .with_source(
                    spiral_robotics_common::datasets::DatasetSelection::new(vec![
                        "dataset_warp_sim_bottles_sample".into(),
                    ])
                    .unwrap(),
                );
        proposal.apply(&mut spec, "automatic").unwrap();
        assert_eq!(spec.transforms().len(), 1);
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
