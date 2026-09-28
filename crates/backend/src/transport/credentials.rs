use spiral_robotics_common::providers::{LlmProvider, StudioProviders};
use spiral_robotics_storage::ProviderKeys;
use url::Url;

const LLM_BASE_URL: &str = "https://openrouter.ai/api/v1";
const HUGGING_FACE: &str = "https://huggingface.co";

/// The provider keys Studio holds. They never reach the browser: the relay
/// attaches each one to requests bound for its provider's origin.
pub(super) struct ProviderCredentials {
    /// A malformed provider override only fails what needs the provider.
    llm: Result<Option<LlmCredentials>, String>,
    hugging_face: Option<String>,
}

impl ProviderCredentials {
    /// The saved keys, and the provider endpoint the environment may override.
    pub(super) fn load(keys: &ProviderKeys) -> Self {
        Self {
            llm: Self::llm(keys.llm_api_key(), |name| std::env::var(name).ok()),
            hugging_face: keys.huggingface_token().map(str::to_owned),
        }
    }

    /// What the browser may know about the providers.
    pub(super) fn providers(&self) -> Result<StudioProviders, String> {
        let llm = self.llm.clone()?.map(|llm| LlmProvider::new(llm.base_url));
        Ok(StudioProviders::new(llm))
    }

    /// The bearer token Studio holds for `url`'s origin, if any.
    pub(super) fn bearer_for(&self, url: &Url) -> Option<&str> {
        let same_origin =
            |base: &str| Url::parse(base).is_ok_and(|base| base.origin() == url.origin());
        if let Ok(Some(llm)) = &self.llm
            && same_origin(&llm.base_url)
        {
            return Some(&llm.api_key);
        }
        self.hugging_face
            .as_deref()
            .filter(|_| same_origin(HUGGING_FACE))
    }

    /// The model provider for the saved key. `LLM_BASE_URL` remains a
    /// developer override of the endpoint.
    fn llm(
        api_key: Option<&str>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<LlmCredentials>, String> {
        let Some(api_key) = api_key else {
            return Ok(None);
        };
        let override_of = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let base_url = override_of("LLM_BASE_URL").unwrap_or_else(|| LLM_BASE_URL.into());
        let parsed = Url::parse(&base_url)
            .map_err(|error| format!("Model provider endpoint is invalid: {error}"))?;
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return Err(
                "Model provider endpoint must use HTTPS (HTTP is allowed only for localhost)."
                    .into(),
            );
        }
        Ok(Some(LlmCredentials {
            base_url: parsed.as_str().trim_end_matches('/').into(),
            api_key: api_key.to_owned(),
        }))
    }
}

#[derive(Clone)]
struct LlmCredentials {
    base_url: String,
    api_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::providers::ProviderKeysUpdate;
    use std::collections::BTreeMap;

    fn keys(llm_api_key: Option<&str>, huggingface_token: Option<&str>) -> ProviderKeys {
        ProviderKeys::default().updated(
            &ProviderKeysUpdate::default()
                .with_llm_api_key(llm_api_key.map(str::to_owned))
                .with_huggingface_token(huggingface_token.map(str::to_owned)),
        )
    }

    #[test]
    fn the_saved_key_uses_defaults_unless_the_environment_overrides_them() {
        let saved = keys(Some("saved-secret"), None);
        let config = ProviderCredentials::llm(saved.llm_api_key(), |_| None)
            .unwrap()
            .unwrap();
        assert_eq!(config.api_key, "saved-secret");
        assert_eq!(config.base_url, LLM_BASE_URL);
        let overrides = BTreeMap::from([("LLM_BASE_URL", "https://custom.example/v1/")]);
        let config = ProviderCredentials::llm(saved.llm_api_key(), |name| {
            overrides.get(name).map(ToString::to_string)
        })
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url, "https://custom.example/v1");
        assert!(
            ProviderCredentials::llm(None, |name| overrides.get(name).map(ToString::to_string))
                .unwrap()
                .is_none()
        );
        let insecure = ProviderCredentials::llm(saved.llm_api_key(), |name| {
            (name == "LLM_BASE_URL").then(|| "http://provider.example/v1".to_owned())
        });
        assert!(insecure.err().unwrap().contains("HTTPS"));
    }

    #[test]
    fn keys_attach_only_to_their_providers_origin() {
        let credentials = ProviderCredentials {
            llm: Ok(Some(LlmCredentials {
                base_url: "https://openrouter.ai/api/v1".into(),
                api_key: "llm-key".into(),
            })),
            hugging_face: Some("hf-token".into()),
        };
        let bearer = |url: &str| credentials.bearer_for(&Url::parse(url).unwrap());
        assert_eq!(
            bearer("https://openrouter.ai/api/alpha/decisions"),
            Some("llm-key")
        );
        assert_eq!(
            bearer("https://huggingface.co/org/model/resolve/main/x"),
            Some("hf-token")
        );
        assert_eq!(bearer("http://openrouter.ai/api/v1/chat/completions"), None);
        assert_eq!(bearer("https://openrouter.ai.example/api/v1"), None);
        assert_eq!(bearer("https://raw.githubusercontent.com/robot.urdf"), None);
        let providers = serde_json::to_string(&credentials.providers().unwrap()).unwrap();
        assert!(providers.contains("openrouter.ai") && !providers.contains("llm-key"));
        let broken = ProviderCredentials {
            llm: Err("Model provider endpoint is invalid".into()),
            hugging_face: Some("hf-token".into()),
        };
        assert!(broken.providers().is_err());
        let url = Url::parse("https://huggingface.co/model").unwrap();
        assert_eq!(broken.bearer_for(&url), Some("hf-token"));
        let hugging_face_only = ProviderCredentials::load(&keys(None, Some("hf-saved")));
        assert!(hugging_face_only.providers().unwrap().llm().is_none());
        assert_eq!(hugging_face_only.bearer_for(&url), Some("hf-saved"));
    }
}
