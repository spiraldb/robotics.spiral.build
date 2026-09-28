//! Browser-owned chat completions: one provider request, decoded into the
//! assistant message and tool calls it carried.
use super::proxy::ProxyClient;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::providers::{LlmProvider, ProviderMessage, ProviderToolCall};
use std::collections::BTreeSet;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// How much a provider may think before answering. Providers reject
/// disabling reasoning outright, so there is no "none".
const REASONING_EFFORT: &str = "medium";

pub(crate) struct ModelRequest {
    messages: Vec<ProviderMessage>,
    tools: Vec<Value>,
    model: String,
}
impl ModelRequest {
    pub(crate) fn new(
        messages: Vec<ProviderMessage>,
        tools: Vec<Value>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            messages,
            tools,
            model: model.into(),
        }
    }
}

/// The chat-completions provider the operator configured. Its requests go
/// through `/proxy`, so they name no project; a view that stops caring
/// cancels them by unmounting.
pub(crate) struct ModelClient;
impl ModelClient {
    pub(crate) async fn complete(request: ModelRequest) -> Result<ProviderMessage, String> {
        let providers = ProxyClient::providers().await?;
        let llm = providers
            .llm()
            .ok_or("The model provider is not configured.")?;
        let endpoint = format!("{}/chat/completions", llm.base_url().trim_end_matches('/'));
        let model = Self::provider_model(llm, &request.model);
        let mut payload = json!({"model":model,"reasoning_effort":REASONING_EFFORT,"service_tier":"priority","messages":request.messages});
        // A request without tools omits the tool fields: providers that accept
        // images and a response schema reject `tool_choice` with no tools.
        if !request.tools.is_empty() {
            payload["tools"] = Value::Array(request.tools);
            payload["tool_choice"] = json!("auto");
        }
        let body = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
        let response = ProxyClient::text(
            ProxyClient::request("POST", &endpoint)
                .header("content-type", "application/json")
                .raw(body.into_bytes())
                .timeout(90_000),
            MAX_RESPONSE_BYTES,
        )
        .await?;
        CompletionResponse::decode(&response).map_err(|error| error.chars().take(512).collect())
    }
    fn provider_model(provider: &LlmProvider, model: &str) -> String {
        if provider.base_url().starts_with("https://openrouter.ai/") && !model.contains('/') {
            format!("openai/{model}")
        } else {
            model.to_owned()
        }
    }
}

/// A chat-completions response body, read leniently: providers add fields of
/// their own beside the ones Studio needs.
#[derive(Deserialize)]
struct CompletionResponse {
    #[serde(default)]
    choices: Vec<CompletionChoice>,
    #[serde(default)]
    error: Option<Value>,
}
#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct CompletionMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<CompletionToolCall>>,
}
#[derive(Deserialize)]
struct CompletionToolCall {
    id: String,
    function: CompletionFunction,
}
#[derive(Deserialize)]
struct CompletionFunction {
    name: String,
    arguments: String,
}

impl CompletionResponse {
    fn decode(body: &str) -> Result<ProviderMessage, String> {
        let response: Self = serde_json::from_str(body)
            .map_err(|error| format!("The LLM provider returned a malformed response: {error}"))?;
        if let Some(error) = response.error {
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The provider reported a generation error.")
                .to_owned());
        }
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or("The LLM provider returned no choices.")?;
        if let Some(reason) = choice.finish_reason.as_deref()
            && !matches!(reason, "stop" | "tool_calls")
        {
            return Err(format!(
                "The LLM provider did not complete the response ({reason})."
            ));
        }
        let mut ids = BTreeSet::new();
        let calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|call| {
                if call.id.is_empty()
                    || call.function.name.is_empty()
                    || !ids.insert(call.id.clone())
                    || !serde_json::from_str::<Value>(&call.function.arguments)
                        .is_ok_and(|value| value.is_object())
                {
                    return Err(
                        "The LLM provider returned an incomplete or invalid tool call.".to_owned(),
                    );
                }
                Ok(ProviderToolCall::new(
                    call.id,
                    call.function.name,
                    call.function.arguments,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let content = choice.message.content.filter(|content| !content.is_empty());
        if content.is_none() && calls.is_empty() {
            return Err("The LLM provider completed without a message or tool call.".into());
        }
        Ok(ProviderMessage::assistant(content, calls))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OpenRouter response as it arrives, with the provider's own fields.
    const TOOL_CALL_RESPONSE: &str = r#"{"id":"gen-1759080000-abc","provider":"OpenAI","model":"openai/gpt-5.6-terra","object":"chat.completion","created":1759080000,"choices":[{"logprobs":null,"finish_reason":"tool_calls","native_finish_reason":"tool_calls","index":0,"message":{"role":"assistant","content":"","refusal":null,"reasoning":"Read the stream first.","tool_calls":[{"id":"call_1","index":0,"type":"function","function":{"name":"inspect_stream","arguments":"{\"topic\":\"/camera\",\"schema\":\"foxglove.CompressedImage\"}"}}]}}],"usage":{"prompt_tokens":812,"completion_tokens":64,"total_tokens":876}}"#;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_response_decodes_into_its_message_and_tool_calls() {
        let message = CompletionResponse::decode(TOOL_CALL_RESPONSE).unwrap();
        assert_eq!(message.content(), None);
        let [call] = message.tool_calls() else {
            panic!("one tool call");
        };
        assert_eq!(call.id(), "call_1");
        assert_eq!(call.function().name(), "inspect_stream");
        let text = r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"fold\nflatten","tool_calls":null}}]}"#;
        assert_eq!(
            CompletionResponse::decode(text).unwrap().content(),
            Some("fold\nflatten")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn truncated_errored_and_invalid_responses_are_refused() {
        let truncated = r#"{"choices":[{"finish_reason":"length","message":{"content":"fo"}}]}"#;
        assert!(
            CompletionResponse::decode(truncated)
                .unwrap_err()
                .contains("length")
        );
        let errored = r#"{"error":{"message":"Rate limited","code":429}}"#;
        assert_eq!(
            CompletionResponse::decode(errored).unwrap_err(),
            "Rate limited"
        );
        let invalid = r#"{"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call_1","type":"function","function":{"name":"inspect_stream","arguments":"[]"}}]}}]}"#;
        assert!(CompletionResponse::decode(invalid).is_err());
    }
}
