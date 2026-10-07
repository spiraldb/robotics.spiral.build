//! The keys Studio's deployment gives it for its providers: the OpenRouter
//! key, the Hugging Face token, the address of each Modal service — the one
//! that scores progress, the one that tracks objects, and the one that
//! trains — and the one token all of them accept.
//! They come from the process environment, so no user sees or sets them. The
//! backend attaches them to its own requests and hands the first two to every
//! database it opens.

/// The provider keys the process started with.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ProviderKeys {
    llm_api_key: Option<String>,
    huggingface_token: Option<String>,
    progress_service_url: Option<String>,
    object_tracking_url: Option<String>,
    training_service_url: Option<String>,
    modal_token: Option<String>,
}

impl ProviderKeys {
    const LLM_API_KEY: &str = "OPENROUTER_API_KEY";
    const HUGGINGFACE_TOKEN: &str = "HF_TOKEN";
    const PROGRESS_SERVICE_URL: &str = "WARP_PROGRESS_URL";
    const OBJECT_TRACKING_URL: &str = "OBJECT_TRACKING_URL";
    const TRAINING_SERVICE_URL: &str = "TRAINING_SERVICE_URL";
    const MODAL_TOKEN: &str = "MODAL_SERVICE_TOKEN";

    /// The keys in this process's environment.
    pub fn from_environment() -> Self {
        Self::from_variables(|name| std::env::var(name).ok())
    }

    /// The keys `variable` gives for their environment variables; a blank
    /// variable is no key.
    pub fn from_variables(variable: impl Fn(&str) -> Option<String>) -> Self {
        let key = |name: &str| {
            variable(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Self {
            llm_api_key: key(Self::LLM_API_KEY),
            huggingface_token: key(Self::HUGGINGFACE_TOKEN),
            progress_service_url: key(Self::PROGRESS_SERVICE_URL),
            object_tracking_url: key(Self::OBJECT_TRACKING_URL),
            training_service_url: key(Self::TRAINING_SERVICE_URL),
            modal_token: key(Self::MODAL_TOKEN),
        }
    }

    pub fn llm_api_key(&self) -> Option<&str> {
        self.llm_api_key.as_deref()
    }

    pub fn huggingface_token(&self) -> Option<&str> {
        self.huggingface_token.as_deref()
    }

    /// Where the Modal service that scores progress is reached.
    pub fn progress_service_url(&self) -> Option<&str> {
        self.progress_service_url.as_deref()
    }

    /// Where the Modal service that tracks objects is reached.
    pub fn object_tracking_url(&self) -> Option<&str> {
        self.object_tracking_url.as_deref()
    }

    /// Where the Modal service that trains policies is reached.
    pub fn training_service_url(&self) -> Option<&str> {
        self.training_service_url.as_deref()
    }

    /// The bearer token every Modal service of this deployment accepts.
    pub fn modal_token(&self) -> Option<&str> {
        self.modal_token.as_deref()
    }
}

/// Which keys are set, never the keys.
impl std::fmt::Debug for ProviderKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderKeys")
            .field(Self::LLM_API_KEY, &self.llm_api_key.is_some())
            .field(Self::HUGGINGFACE_TOKEN, &self.huggingface_token.is_some())
            .field(
                Self::PROGRESS_SERVICE_URL,
                &self.progress_service_url.is_some(),
            )
            .field(
                Self::OBJECT_TRACKING_URL,
                &self.object_tracking_url.is_some(),
            )
            .field(
                Self::TRAINING_SERVICE_URL,
                &self.training_service_url.is_some(),
            )
            .field(Self::MODAL_TOKEN, &self.modal_token.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn keys_come_from_their_variables_and_blank_ones_are_unset() {
        let variables = BTreeMap::from([
            ("OPENROUTER_API_KEY", " sk-or-private "),
            ("HF_TOKEN", "  "),
            ("WARP_PROGRESS_URL", "https://scores.modal.run"),
            ("OBJECT_TRACKING_URL", "https://tracks.modal.run"),
            ("TRAINING_SERVICE_URL", "https://train.modal.run"),
            ("MODAL_SERVICE_TOKEN", "modal-private"),
        ]);
        let keys =
            ProviderKeys::from_variables(|name| variables.get(name).map(ToString::to_string));
        assert_eq!(keys.llm_api_key(), Some("sk-or-private"));
        assert_eq!(keys.huggingface_token(), None);
        assert_eq!(
            keys.progress_service_url(),
            Some("https://scores.modal.run")
        );
        assert_eq!(keys.object_tracking_url(), Some("https://tracks.modal.run"));
        assert_eq!(keys.modal_token(), Some("modal-private"));
        assert_eq!(keys.training_service_url(), Some("https://train.modal.run"));
        let debug = format!("{keys:?}");
        assert!(!debug.contains("sk-or-private") && !debug.contains("modal-private"));
        assert!(debug.contains("OBJECT_TRACKING_URL: true") && !debug.contains("tracks.modal.run"));
        assert_eq!(
            ProviderKeys::from_variables(|_| None),
            ProviderKeys::default()
        );
    }
}
