//! Asking jev, through OpenRouter's Decisions API: what a question looks
//! like, how one is sent, and how its answer is read.
//!
//! A request is one `state` — the material every question judges — and a map
//! of question id to [`DecisionQuestion`]. jev evaluates them in parallel and
//! answers each under its id, which is what [`DecisionResponse`] reads back.
use super::{CommandError, completions::ModelClient, proxy::ProxyClient};
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::command_failure::CommandFailureCode;
use std::collections::BTreeMap;

/// The one model Studio asks for a decision. Curation interprets a rule with
/// it; Explore ranks and categorizes subtasks with it.
const DECISION_MODEL: &str = "typesafe/jev-1.13";

/// One question of a Decisions request: what to judge, and the words that
/// tell its answers apart.
pub(crate) struct DecisionQuestion(Value);

impl DecisionQuestion {
    /// Pick one of `criteria`. The key is what the caller reads back from
    /// [`DecisionResponse::choice`]; the value is what that option means.
    pub(crate) fn choice(
        instructions: impl Into<String>,
        criteria: BTreeMap<String, String>,
    ) -> Self {
        Self(json!({"type": "choice", "instructions": instructions.into(), "criteria": criteria}))
    }

    /// How probable it is that `instructions` holds, with the two criteria
    /// that say what a true and a false answer mean.
    pub(crate) fn noul(instructions: impl Into<String>, when_true: &str, when_false: &str) -> Self {
        Self(json!({
            "type": "noul",
            "instructions": instructions.into(),
            "criteria": {"true": when_true, "false": when_false},
        }))
    }
}

impl From<DecisionQuestion> for Value {
    fn from(question: DecisionQuestion) -> Self {
        question.0
    }
}

impl ModelClient {
    /// Judge `state` against `questions`, keyed by question id.
    pub(crate) async fn decide(
        state: Value,
        questions: Value,
    ) -> Result<DecisionResponse, CommandError> {
        let providers = ProxyClient::providers().await?;
        let llm = providers.llm().ok_or_else(|| {
            CommandError::new(
                CommandFailureCode::InvalidInput,
                "Configure the Assistant's OpenRouter credential to interpret a rule.",
            )
        })?;
        if !llm.base_url().starts_with("https://openrouter.ai/") {
            return Err(CommandError::new(
                CommandFailureCode::InvalidInput,
                "Rule interpretation requires an OpenRouter provider configuration.",
            ));
        }
        let request = ProxyClient::request("POST", "https://openrouter.ai/api/alpha/decisions")
            .json(&json!({"model":DECISION_MODEL, "state":state, "questions":questions}))
            .timeout(30_000);
        let text = ProxyClient::text(request, 128 * 1024).await?;
        serde_json::from_str(&text).map_err(|_| {
            CommandError::transport("OpenRouter returned an invalid Decisions response.")
        })
    }
}

/// Answers to one Decisions request, keyed by question id. Three readers:
/// `choice` for decisions code acts on (confidence-gated), `noul` for a
/// calibrated yes probability, and `choice_distribution` for rankings and
/// categorization that read every option's probability without a gate.
#[derive(Deserialize)]
pub(crate) struct DecisionResponse {
    answers: BTreeMap<String, DecisionAnswer>,
}
impl DecisionResponse {
    /// A Choice answer the caller may act on: confidence at least 0.5.
    pub(crate) fn choice(&self, question: &str) -> Option<&str> {
        let answer = self.answers.get(question)?;
        (answer.kind == "choice"
            && answer
                .confidence
                .is_some_and(|value| value.is_finite() && (0.5..=1.0).contains(&value)))
        .then_some(answer.choice.as_deref()?)
    }
    /// A Noul answer: the probability, 0 to 1, that the question is true.
    pub(crate) fn noul(&self, question: &str) -> Option<f64> {
        let answer = self.answers.get(question)?;
        (answer.kind == "noul")
            .then_some(answer.noul?)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
    }
    /// A Choice answer's whole distribution, ungated.
    pub(crate) fn choice_distribution(&self, question: &str) -> Option<ChoiceDistribution<'_>> {
        let answer = self.answers.get(question)?;
        if answer.kind != "choice" || answer.probabilities.is_empty() {
            return None;
        }
        if answer
            .probabilities
            .values()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return None;
        }
        Some(ChoiceDistribution {
            choice: answer.choice.as_deref()?,
            probabilities: &answer.probabilities,
            confidence: answer
                .confidence
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value)),
        })
    }
}
/// The winning option, the probability of every option (they sum to 1), and
/// how peaked the distribution is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChoiceDistribution<'a> {
    choice: &'a str,
    probabilities: &'a BTreeMap<String, f64>,
    confidence: Option<f64>,
}
impl ChoiceDistribution<'_> {
    pub(crate) fn choice(&self) -> &str {
        self.choice
    }
    pub(crate) fn probability(&self, option: &str) -> f64 {
        self.probabilities.get(option).copied().unwrap_or(0.0)
    }
    /// The most probable option other than the winner, when there is one.
    pub(crate) fn runner_up(&self) -> Option<(&str, f64)> {
        self.probabilities
            .iter()
            .filter(|(option, _)| option.as_str() != self.choice)
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(option, probability)| (option.as_str(), *probability))
    }
    pub(crate) fn confidence(&self) -> Option<f64> {
        self.confidence
    }
}
#[derive(Deserialize)]
struct DecisionAnswer {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    probabilities: BTreeMap<String, f64>,
    #[serde(default)]
    noul: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(json: &str) -> DecisionResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn choice_stays_gated_while_the_distribution_is_not() {
        let response = response(
            r#"{"answers":{"category":{"type":"choice","choice":"fold","confidence":0.2,"probabilities":{"fold":0.4,"grasp":0.35,"smooth":0.25}}},"usage":{"input_tokens":9}}"#,
        );
        assert_eq!(response.choice("category"), None);
        let distribution = response.choice_distribution("category").unwrap();
        assert_eq!(distribution.choice(), "fold");
        assert_eq!(distribution.probability("grasp"), 0.35);
        assert_eq!(distribution.probability("missing"), 0.0);
        assert_eq!(distribution.confidence(), Some(0.2));
        assert_eq!(distribution.runner_up(), Some(("grasp", 0.35)));
    }

    #[test]
    fn noul_reads_only_finite_probabilities_of_its_own_type() {
        let response = response(
            r#"{"answers":{"r1":{"type":"noul","noul":0.83},"r2":{"type":"noul","noul":1.5},"r3":{"type":"choice","choice":"x","confidence":1.0}}}"#,
        );
        assert_eq!(response.noul("r1"), Some(0.83));
        assert_eq!(response.noul("r2"), None);
        assert_eq!(response.noul("r3"), None);
        assert_eq!(response.choice("r1"), None);
        assert_eq!(response.choice("r3"), Some("x"));
        assert_eq!(response.choice_distribution("r3"), None);
    }

    #[test]
    fn a_question_carries_its_kind_its_instructions_and_its_criteria() {
        let choice = Value::from(DecisionQuestion::choice(
            "Which one?",
            [("a".to_owned(), "the first".to_owned())]
                .into_iter()
                .collect(),
        ));
        assert_eq!(choice["type"], "choice");
        assert_eq!(choice["instructions"], "Which one?");
        assert_eq!(choice["criteria"]["a"], "the first");
        let noul = Value::from(DecisionQuestion::noul("Same operation?", "yes", "no"));
        assert_eq!(noul["type"], "noul");
        assert_eq!(noul["criteria"]["false"], "no");
    }
}
