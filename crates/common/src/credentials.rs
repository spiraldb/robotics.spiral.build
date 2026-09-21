//! Studio-wide credentials intentionally supplied to the browser at runtime.

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StudioCredentials {
    llm: Option<LlmCredentials>,
    spiral: Option<SpiralCredentials>,
    hugging_face: Option<HuggingFaceCredentials>,
}

impl StudioCredentials {
    pub fn new(
        llm: Option<LlmCredentials>,
        spiral: Option<SpiralCredentials>,
        hugging_face: Option<HuggingFaceCredentials>,
    ) -> Self {
        Self {
            llm,
            spiral,
            hugging_face,
        }
    }
    pub fn llm(&self) -> Option<&LlmCredentials> {
        self.llm.as_ref()
    }
    pub fn hugging_face(&self) -> Option<&HuggingFaceCredentials> {
        self.hugging_face.as_ref()
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmCredentials {
    base_url: String,
    api_key: String,
    default_model: String,
}
impl LlmCredentials {
    pub fn new(base_url: String, api_key: String, default_model: String) -> Self {
        Self {
            base_url,
            api_key,
            default_model,
        }
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
    pub fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpiralCredentials {
    endpoint: String,
    spfs_endpoint: String,
    client_id: String,
    client_secret: String,
}
impl SpiralCredentials {
    pub fn new(
        endpoint: String,
        spfs_endpoint: String,
        client_id: String,
        client_secret: String,
    ) -> Self {
        Self {
            endpoint,
            spfs_endpoint,
            client_id,
            client_secret,
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HuggingFaceCredentials {
    token: String,
}
impl HuggingFaceCredentials {
    pub fn new(token: String) -> Self {
        Self { token }
    }
    pub fn token(&self) -> &str {
        &self.token
    }
}
