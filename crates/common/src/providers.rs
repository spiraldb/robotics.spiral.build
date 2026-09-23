//! Studio-wide model providers as the browser sees them. Their credentials
//! stay on the backend, which attaches them when it relays a request.

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
