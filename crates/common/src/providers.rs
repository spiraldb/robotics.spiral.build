//! Studio-wide model providers as the browser sees them, and the messages it
//! exchanges with them. Their credentials stay on the backend, which attaches
//! them when it relays a request or calls the Modal service itself.

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
}

impl LlmProvider {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Which provider keys the backend holds. The keys themselves never leave it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeysStatus {
    llm_api_key_configured: bool,
    huggingface_token_configured: bool,
    modal_base_url_configured: bool,
    modal_token_configured: bool,
}

impl ProviderKeysStatus {
    pub const fn new(llm_api_key_configured: bool, huggingface_token_configured: bool) -> Self {
        Self {
            llm_api_key_configured,
            huggingface_token_configured,
            modal_base_url_configured: false,
            modal_token_configured: false,
        }
    }
    /// The same, saying whether the Modal service's base URL and token are
    /// saved.
    pub const fn with_modal(mut self, base_url_configured: bool, token_configured: bool) -> Self {
        self.modal_base_url_configured = base_url_configured;
        self.modal_token_configured = token_configured;
        self
    }
    pub const fn llm_api_key_configured(&self) -> bool {
        self.llm_api_key_configured
    }
    pub const fn huggingface_token_configured(&self) -> bool {
        self.huggingface_token_configured
    }
    pub const fn modal_base_url_configured(&self) -> bool {
        self.modal_base_url_configured
    }
    pub const fn modal_token_configured(&self) -> bool {
        self.modal_token_configured
    }
}

/// New values for the provider keys. For each key, omitted or `null` keeps the
/// saved key and an empty string removes it.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeysUpdate {
    #[serde(default)]
    llm_api_key: Option<String>,
    #[serde(default)]
    huggingface_token: Option<String>,
    #[serde(default)]
    modal_base_url: Option<String>,
    #[serde(default)]
    modal_token: Option<String>,
}

impl ProviderKeysUpdate {
    pub fn with_llm_api_key(mut self, key: Option<String>) -> Self {
        self.llm_api_key = key;
        self
    }
    pub fn with_huggingface_token(mut self, token: Option<String>) -> Self {
        self.huggingface_token = token;
        self
    }
    pub fn with_modal_base_url(mut self, base_url: Option<String>) -> Self {
        self.modal_base_url = base_url;
        self
    }
    pub fn with_modal_token(mut self, token: Option<String>) -> Self {
        self.modal_token = token;
        self
    }
    pub fn llm_api_key(&self) -> Option<&str> {
        self.llm_api_key.as_deref()
    }
    pub fn huggingface_token(&self) -> Option<&str> {
        self.huggingface_token.as_deref()
    }
    pub fn modal_base_url(&self) -> Option<&str> {
        self.modal_base_url.as_deref()
    }
    pub fn modal_token(&self) -> Option<&str> {
        self.modal_token.as_deref()
    }
}

impl std::fmt::Debug for ProviderKeysUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |key: &Option<String>| key.as_ref().map(|_| "<redacted>");
        f.debug_struct("ProviderKeysUpdate")
            .field("llm_api_key", &redacted(&self.llm_api_key))
            .field("huggingface_token", &redacted(&self.huggingface_token))
            .field("modal_base_url", &redacted(&self.modal_base_url))
            .field("modal_token", &redacted(&self.modal_token))
            .finish()
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
    fn key_updates_redact_their_values() {
        let update = ProviderKeysUpdate::default()
            .with_llm_api_key(Some("sk-or-private".into()))
            .with_huggingface_token(Some("hf_private".into()))
            .with_modal_token(Some("modal_private".into()));
        let debug = format!("{update:?}");
        assert!(
            !debug.contains("sk-or-private")
                && !debug.contains("hf_private")
                && !debug.contains("modal_private")
        );
        let omitted: ProviderKeysUpdate = serde_json::from_str("{}").unwrap();
        assert_eq!(omitted, ProviderKeysUpdate::default());
    }

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
