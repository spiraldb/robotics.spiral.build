//! Bounded semantic binding through OpenRouter's Decisions API.
use crate::workflow::provider::ProviderConfig;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::curation::{
    interpretation::{InterpretationRequest, NumberCandidate, RuleIntent, RuleProposal},
    rules::ConditionOp,
};
use std::{collections::BTreeMap, time::Duration};

pub(super) struct RuleInterpreter;
impl RuleInterpreter {
    pub(super) async fn interpret(request: InterpretationRequest) -> Result<RuleProposal, String> {
        request.validate()?;
        let numbers = NumberCandidate::extract(request.prompt());
        let questions = Self::questions(&request, &numbers);
        let config = ProviderConfig::from_env()?
            .ok_or("Configure the Assistant's OpenRouter credential to interpret a rule.")?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Could not create the rule interpretation client.")?;
        let response = config.decisions_request(&client)?.json(&json!({
            "model": "typesafe/jev-1.13", "state": request.prompt(), "questions": questions
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
        Ok(Self::bind(&request, &numbers, answers))
    }
    fn choice(instructions: &str, criteria: BTreeMap<String, String>) -> Value {
        let mut criteria = criteria;
        criteria.insert(
            "unresolved".into(),
            "Not specified, ambiguous, unsupported, or none of these choices.".into(),
        );
        json!({"type": "choice", "instructions": instructions, "criteria": criteria})
    }
    fn questions(request: &InterpretationRequest, numbers: &[NumberCandidate]) -> Value {
        json!({
            "operation": Self::choice("Identify the single requested curation operation. Multiple independent operations must be unresolved.", RuleIntent::ALL.into_iter().filter(|intent| *intent != RuleIntent::Unresolved).map(|intent| (intent.key().into(), intent.label().into())).collect()),
            "comparison": Self::choice("For a Filter, when should an ENTIRE EPISODE be EXCLUDED? Convert a keep condition to its complement. Do not invent a threshold. For transforms choose unresolved.", [("below", "Exclude when strictly less than the threshold"), ("at_most", "Exclude when less than or equal to the threshold"), ("above", "Exclude when strictly greater than the threshold"), ("at_least", "Exclude when greater than or equal to the threshold")].into_iter().map(|(key, label)| (key.into(), label.into())).collect()),
            "number": Self::choice("Which exact number in the request is the filter threshold or minimum mean signed WARP velocity? Ignore frame indices, camera numbers and examples. If absent or ambiguous choose unresolved.", numbers.iter().enumerate().map(|(index, number)| (format!("n{index}"), number.text().to_owned())).collect()),
            "unit": Self::choice("What unit is explicitly given for the selected duration threshold? Do not infer a missing unit.", [("seconds", "Seconds, s, sec"), ("minutes", "Minutes, min"), ("milliseconds", "Milliseconds, ms"), ("none", "A count or dimensionless luminance, not a duration")].into_iter().map(|(key, label)| (key.into(), label.into())).collect()),
            "camera": Self::choice("Which exact listed camera does the request identify? If no camera is specified or multiple match, choose unresolved.", request.cameras().iter().enumerate().map(|(index, camera)| (format!("c{index}"), camera.clone())).collect())
        })
    }
    fn bind(
        request: &InterpretationRequest,
        numbers: &[NumberCandidate],
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
        let mut value = response
            .choice("number")
            .and_then(|key| key.strip_prefix('n'))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| numbers.get(index))
            .map(NumberCandidate::value);
        if intent == RuleIntent::Duration {
            let multiplier = match response.choice("unit") {
                Some("seconds") => Some(1.0),
                Some("minutes") => Some(60.0),
                Some("milliseconds") => Some(0.001),
                _ => None,
            };
            if let Some(multiplier) = multiplier {
                value = value.map(|value| value * multiplier);
                proposal.set_unit(Some("seconds".into()));
            }
        }
        proposal.set_value(value);
        let camera = response
            .choice("camera")
            .and_then(|key| key.strip_prefix('c'))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| request.cameras().get(index))
            .cloned();
        proposal.set_camera(camera);
        if intent == RuleIntent::CameraLuminance {
            proposal.add_note("Measures linear relative luminance from frame 0 of the selected camera. Edit sampled frames in Measurements.");
        }
        proposal
    }
}
#[derive(Deserialize)]
struct DecisionResponse {
    answers: BTreeMap<String, ChoiceAnswer>,
}
impl DecisionResponse {
    fn choice(&self, question: &str) -> Option<&str> {
        let answer = self.answers.get(question)?;
        (answer.kind == "choice"
            && answer.confidence.is_some_and(|confidence| {
                confidence.is_finite() && (0.5..=1.0).contains(&confidence)
            }))
        .then_some(answer.choice.as_str())
    }
}
#[derive(Deserialize)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    confidence: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "uses the configured OpenRouter credential for a bounded live Decisions request"]
    async fn live_jev_binds_explicit_filter_and_leaves_vague_threshold_unresolved() {
        let proposal = RuleInterpreter::interpret(InterpretationRequest::new(
            "Exclude episodes shorter than 2 minutes".into(),
            vec!["/cameras/top".into()],
        ))
        .await
        .unwrap();
        assert_eq!(proposal.intent(), RuleIntent::Duration);
        assert_eq!(proposal.comparison(), Some(ConditionOp::Below));
        assert_eq!(proposal.value(), Some(120.0));
        assert!(proposal.problems().is_empty(), "{:?}", proposal.problems());
        let vague = RuleInterpreter::interpret(InterpretationRequest::new(
            "Exclude short episodes".into(),
            Vec::new(),
        ))
        .await
        .unwrap();
        assert_eq!(vague.value(), None);
        assert!(!vague.problems().is_empty());
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
            "number": {"type":"choice","choice":"n99","confidence":1.0},
            "camera": {"type":"choice","choice":"/invented","confidence":1.0},
            "unit": {"type":"choice","choice":"minutes","confidence":1.0}
        }}))
        .unwrap();
        let proposal = RuleInterpreter::bind(
            &request,
            &NumberCandidate::extract(request.prompt()),
            answers,
        );
        assert_eq!(proposal.value(), None);
        assert_eq!(proposal.camera(), None);
        assert!(!proposal.problems().is_empty());
    }
}
