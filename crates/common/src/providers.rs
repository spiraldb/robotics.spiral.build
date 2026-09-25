//! Studio-wide model providers as the browser sees them, and the messages it
//! exchanges with them. Their credentials stay on the backend, which attaches
//! them when it relays a request.

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StudioProviders {
    llm: Option<LlmProvider>,
}

impl StudioProviders {
    pub fn new(llm: Option<LlmProvider>) -> Self {
        Self { llm }
    }
    pub fn llm(&self) -> Option<&LlmProvider> {
        self.llm.as_ref()
    }
}

/// The configured chat-completions provider. The relay authenticates every
/// request to its origin.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmProvider {
    base_url: String,
    default_model: String,
}

impl LlmProvider {
    pub fn new(base_url: String, default_model: String) -> Self {
        Self {
            base_url,
            default_model,
        }
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn default_model(&self) -> &str {
        &self.default_model
    }
}

/// One chat-completions message as the provider reads and writes it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMessage {
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ProviderToolCall>,
}

impl ProviderMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool(call_id: String, content: String) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(content),
            tool_call_id: Some(call_id),
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ProviderToolCall>) -> Self {
        Self {
            role: "assistant".to_owned(),
            content,
            tool_call_id: None,
            tool_calls,
        }
    }

    pub fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }
    pub fn tool_calls(&self) -> &[ProviderToolCall] {
        &self.tool_calls
    }
}

/// One function call the provider asked for in an assistant message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: ProviderFunctionCall,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderFunctionCall {
    name: String,
    arguments: String,
}

impl ProviderToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_owned(),
            function: ProviderFunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn function(&self) -> &ProviderFunctionCall {
        &self.function
    }
}

impl ProviderFunctionCall {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn arguments(&self) -> &str {
        &self.arguments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_travels_as_the_content_string() {
        let message = ProviderMessage::text("user", "hello");
        let encoded = serde_json::to_value(&message).unwrap();
        assert_eq!(encoded["content"], "hello");
        assert_eq!(
            serde_json::from_value::<ProviderMessage>(encoded).unwrap(),
            message
        );
        assert_eq!(message.content(), Some("hello"));
    }

    #[test]
    fn tool_calls_round_trip_in_the_provider_shape() {
        let call = ProviderToolCall::new("call-a", "finish", "{}");
        let message = ProviderMessage::assistant(None, vec![call.clone()]);
        let encoded = serde_json::to_value(&message).unwrap();
        assert_eq!(encoded["tool_calls"][0]["type"], "function");
        assert_eq!(encoded["tool_calls"][0]["function"]["name"], "finish");
        assert_eq!(message.tool_calls(), [call]);
        let reply =
            serde_json::to_value(ProviderMessage::tool("call-a".into(), "ok".into())).unwrap();
        assert_eq!(reply["tool_call_id"], "call-a");
    }
}
