//! Browser-owned provider requests, streaming decode, and retry decisions.
use super::{CommandError, project::ProjectClient, proxy::ProxyClient};
use eventsource_stream::Eventsource as _;
use futures_util::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    command_failure::CommandFailureCode,
    credentials::LlmCredentials,
    workflow::conversation::{ProviderMessage, ProviderToolCall},
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_STREAM_TOOL_CALLS: usize = 24;
const MAX_ATTEMPTS: usize = 3;

pub(crate) struct ModelRequest {
    messages: Vec<ProviderMessage>,
    tools: Vec<Value>,
    model: Option<String>,
}
impl ModelRequest {
    pub(crate) fn new(messages: Vec<ProviderMessage>, tools: Vec<Value>) -> Self {
        Self {
            messages,
            tools,
            model: None,
        }
    }
    pub(crate) fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
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
    // Read by Build's explorations; the allow goes away when they land.
    #[allow(dead_code)]
    pub(crate) fn noul(&self, question: &str) -> Option<f64> {
        let answer = self.answers.get(question)?;
        (answer.kind == "noul")
            .then_some(answer.noul?)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
    }
    /// A Choice answer's whole distribution, ungated.
    #[allow(dead_code)]
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
#[allow(dead_code)]
impl ChoiceDistribution<'_> {
    pub(crate) fn choice(&self) -> &str {
        self.choice
    }
    pub(crate) fn probability(&self, option: &str) -> f64 {
        self.probabilities.get(option).copied().unwrap_or(0.0)
    }
    pub(crate) fn probabilities(&self) -> &BTreeMap<String, f64> {
        self.probabilities
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

#[derive(Clone)]
pub(crate) struct ModelClient {
    project: ProjectClient,
}
impl ModelClient {
    pub(crate) fn new(project: ProjectClient) -> Self {
        Self { project }
    }
    pub(crate) fn current() -> Self {
        Self::new(ProjectClient::current())
    }

    pub(crate) async fn configured(&self) -> Result<bool, CommandError> {
        self.project.require_current()?;
        let credentials = ProxyClient::credentials().await?;
        self.project.require_current()?;
        Ok(credentials.llm().is_some())
    }

    pub(crate) async fn decide(
        &self,
        model: &str,
        state: Value,
        questions: Value,
    ) -> Result<DecisionResponse, CommandError> {
        self.project.require_current()?;
        let credentials = ProxyClient::credentials().await?;
        self.project.require_current()?;
        let llm = credentials.llm().ok_or_else(|| {
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
            .header("authorization", format!("Bearer {}", llm.api_key()))
            .json(&json!({"model":model, "state":state, "questions":questions}))
            .timeout(30_000);
        let text = ProxyClient::text(request, 128 * 1024).await?;
        self.project.require_current()?;
        serde_json::from_str(&text).map_err(|_| {
            CommandError::transport("OpenRouter returned an invalid Decisions response.")
        })
    }

    pub(crate) async fn complete(
        &self,
        request: ModelRequest,
        mut on_text: impl FnMut(&str),
    ) -> Result<ProviderMessage, String> {
        self.project.require_current()?;
        let credentials = ProxyClient::credentials().await?;
        let llm = credentials
            .llm()
            .ok_or("Assistant provider is not configured.")?;
        let endpoint = format!("{}/chat/completions", llm.base_url().trim_end_matches('/'));
        let model = request
            .model
            .as_deref()
            .map(|model| Self::provider_model(llm, model))
            .unwrap_or_else(|| llm.default_model().to_owned());
        let body = serde_json::to_string(&json!({"model":model,"reasoning_effort":"medium","service_tier":"priority","messages":request.messages,"tools":request.tools,"tool_choice":"auto","stream":true})).map_err(|error| error.to_string())?;
        for attempt in 0..MAX_ATTEMPTS {
            self.project.require_current()?;
            let (response, _cancel) = ProxyClient::request("POST", &endpoint)
                .header("authorization", format!("Bearer {}", llm.api_key()))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .raw(body.clone().into_bytes())
                .timeout(90_000)
                .response("text/event-stream")
                .await?;
            if matches!(response.status(), 429 | 502 | 503) && attempt + 1 < MAX_ATTEMPTS {
                gloo_timers::future::TimeoutFuture::new(250 * (attempt as u32 + 1)).await;
                continue;
            }
            if !(200..300).contains(&response.status()) {
                return Err(format!("LLM provider returned HTTP {}.", response.status()));
            }
            let message = ProviderStream::read(response, &self.project, &mut on_text)
                .await
                .map_err(|error| Self::public_error(&error, llm.api_key()))?;
            self.project.require_current()?;
            return Ok(message);
        }
        Err("The LLM provider remained overloaded after bounded retries.".into())
    }
    fn provider_model(credentials: &LlmCredentials, model: &str) -> String {
        if credentials.base_url().starts_with("https://openrouter.ai/") && !model.contains('/') {
            format!("openai/{model}")
        } else {
            model.to_owned()
        }
    }

    fn public_error(detail: &str, api_key: &str) -> String {
        detail
            .replace(api_key, "[redacted]")
            .chars()
            .take(512)
            .collect()
    }
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}
#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    error: Option<Value>,
}
#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}
#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolDelta>,
}
#[derive(Deserialize)]
struct StreamToolDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunctionDelta>,
}
#[derive(Deserialize)]
struct StreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

struct ProviderStream;
impl ProviderStream {
    async fn read(
        response: gloo_net::http::Response,
        project: &ProjectClient,
        on_text: &mut impl FnMut(&str),
    ) -> Result<ProviderMessage, String> {
        let body = response.body().ok_or("Missing model response body")?;
        let mut received = 0;
        let mut stream = wasm_streams::ReadableStream::from_raw(body)
            .into_stream()
            .map(move |chunk| {
                let chunk = chunk.map_err(|error| format!("Model stream failed: {error:?}"))?;
                let chunk = bytes::Bytes::from(js_sys::Uint8Array::new(&chunk).to_vec());
                received += chunk.len();
                if received > MAX_RESPONSE_BYTES {
                    return Err(
                        "The LLM provider response exceeded the 8 MiB safety limit.".to_owned()
                    );
                }
                Ok(chunk)
            })
            .eventsource();
        let mut content = String::new();
        let mut calls = Vec::<PendingToolCall>::new();
        let mut completed = false;
        while let Some(event) = stream.next().await {
            project.require_current()?;
            let event = event
                .map_err(|error| format!("The LLM provider returned invalid SSE data: {error}"))?;
            let data = event.data.trim();
            if data == "[DONE]" {
                completed = true;
                break;
            }
            let chunk: StreamChunk = serde_json::from_str(data).map_err(|error| {
                format!("The LLM provider returned malformed stream data: {error}")
            })?;
            if let Some(error) = chunk.error {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The provider reported a generation error.")
                    .to_owned());
            }
            for choice in chunk.choices {
                if let Some(reason) = choice.finish_reason {
                    if matches!(reason.as_str(), "stop" | "tool_calls") {
                        completed = true;
                    } else {
                        return Err(format!(
                            "The LLM provider did not complete the response ({reason})."
                        ));
                    }
                }
                if let Some(delta) = choice.delta.content {
                    content.push_str(&delta);
                    on_text(&delta);
                }
                for delta in choice.delta.tool_calls {
                    Self::apply_tool_delta(&mut calls, delta)?;
                }
            }
            if completed {
                break;
            }
        }
        if !completed {
            return Err("The LLM provider stream ended before completing its response.".into());
        }
        let mut ids = BTreeSet::new();
        let calls = calls
            .into_iter()
            .map(|call| {
                if call.id.is_empty()
                    || call.name.is_empty()
                    || !ids.insert(call.id.clone())
                    || !serde_json::from_str::<Value>(&call.arguments)
                        .is_ok_and(|value| value.is_object())
                {
                    return Err(
                        "The LLM provider returned an incomplete or invalid tool call.".to_owned(),
                    );
                }
                Ok(ProviderToolCall::new(call.id, call.name, call.arguments))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if content.is_empty() && calls.is_empty() {
            return Err("The LLM provider completed without a message or tool call.".into());
        }
        Ok(ProviderMessage::assistant(
            (!content.is_empty()).then_some(content),
            calls,
        ))
    }
    fn apply_tool_delta(
        calls: &mut Vec<PendingToolCall>,
        delta: StreamToolDelta,
    ) -> Result<(), String> {
        if delta.index >= MAX_STREAM_TOOL_CALLS {
            return Err(
                "The LLM provider returned a tool-call index outside the safety bound.".into(),
            );
        }
        if calls.len() <= delta.index {
            calls.resize_with(delta.index + 1, PendingToolCall::default);
        }
        let call = &mut calls[delta.index];
        if let Some(id) = delta.id {
            call.id.push_str(&id);
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                call.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                call.arguments.push_str(&arguments);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod decision_reader_tests {
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
}
