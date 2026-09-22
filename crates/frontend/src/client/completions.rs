//! Browser-owned chat completions: one streamed provider request, decoded
//! into the assistant message and tool calls it carried.
use super::{CommandError, project::ProjectClient, proxy::ProxyClient};
use eventsource_stream::Eventsource as _;
use futures_util::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    credentials::LlmCredentials,
    workflow::conversation::{ProviderMessage, ProviderToolCall},
};
use std::collections::BTreeSet;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct ModelRequest {
    messages: Vec<ProviderMessage>,
    tools: Vec<Value>,
    model: Option<String>,
    response_format: Option<Value>,
    reasoning: ReasoningEffort,
}
impl ModelRequest {
    pub(crate) fn new(messages: Vec<ProviderMessage>, tools: Vec<Value>) -> Self {
        Self {
            messages,
            tools,
            model: None,
            response_format: None,
            reasoning: ReasoningEffort::default(),
        }
    }
    pub(crate) fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
    pub(crate) fn with_reasoning(mut self, reasoning: ReasoningEffort) -> Self {
        self.reasoning = reasoning;
        self
    }
    /// Ask the provider to answer in a fixed JSON shape, as a chat-completions
    /// `response_format` value. A provider that ignores it still answers as
    /// text, so callers validate what comes back either way.
    pub(crate) fn with_response_format(mut self, response_format: Value) -> Self {
        self.response_format = Some(response_format);
        self
    }
}

/// How much a provider may think before answering. The chat agent plans SQL
/// and keeps the default; a fixed-schema annotation over pictures does not
/// need it, and Gemini bills thinking as output: on one ABC-130k episode,
/// medium cost $0.059 and 33 s, low $0.0066 and 2.5 s, for comparable
/// subtasks. Providers reject disabling reasoning outright, so there is no
/// "none".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ReasoningEffort {
    Low,
    #[default]
    Medium,
}
impl ReasoningEffort {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
        }
    }
}

/// The provider the operator configured, as this project reaches it: chat
/// completions here, jev Decisions in `client::decisions`.
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
    /// The project every request is checked against, so a sibling transport
    /// abandons its answer when the session moves on.
    pub(in crate::client) const fn project(&self) -> &ProjectClient {
        &self.project
    }

    pub(crate) async fn configured(&self) -> Result<bool, CommandError> {
        self.project.require_current()?;
        let credentials = ProxyClient::credentials().await?;
        self.project.require_current()?;
        Ok(credentials.llm().is_some())
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
        let mut payload = json!({"model":model,"reasoning_effort":request.reasoning.as_str(),"service_tier":"priority","messages":request.messages,"stream":true});
        // A request without tools omits the tool fields: providers that accept
        // images and a response schema reject `tool_choice` with no tools.
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(request.tools);
            payload["tool_choice"] = json!("auto");
        }
        if let Some(response_format) = request.response_format {
            payload["response_format"] = response_format;
        }
        let body = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
        self.project.require_current()?;
        let (response, _cancel) = ProxyClient::request("POST", &endpoint)
            .header("authorization", format!("Bearer {}", llm.api_key()))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .raw(body.into_bytes())
            .timeout(90_000)
            .response("text/event-stream")
            .await?;
        if !(200..300).contains(&response.status()) {
            return Err(format!("LLM provider returned HTTP {}.", response.status()));
        }
        let message = ProviderStream::read(response, &self.project, &mut on_text)
            .await
            .map_err(|error| Self::public_error(&error, llm.api_key()))?;
        self.project.require_current()?;
        Ok(message)
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
                    Self::apply_tool_delta(&mut calls, delta);
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
    /// Tool calls arrive field by field, each delta naming the call it
    /// extends by its index in the response.
    fn apply_tool_delta(calls: &mut Vec<PendingToolCall>, delta: StreamToolDelta) {
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
    }
}
