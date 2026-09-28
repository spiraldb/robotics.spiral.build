//! The keys Studio holds for its providers: the model provider's API key and
//! the Hugging Face token. The backend saves them in its SQLite settings,
//! attaches them to its own requests, and hands them to every database it
//! opens; the browser only learns which are saved.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use spiral_robotics_common::providers::{ProviderKeysStatus, ProviderKeysUpdate};
use tokio::sync::{Mutex, watch};

use crate::settings::BackendStateStore;

/// The saved provider keys. The field names are their durable form in the
/// backend settings document.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    llm_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    huggingface_token: Option<String>,
}

impl ProviderKeys {
    pub fn llm_api_key(&self) -> Option<&str> {
        self.llm_api_key.as_deref()
    }

    pub fn huggingface_token(&self) -> Option<&str> {
        self.huggingface_token.as_deref()
    }

    pub fn status(&self) -> ProviderKeysStatus {
        ProviderKeysStatus::new(self.llm_api_key.is_some(), self.huggingface_token.is_some())
    }

    /// These keys with `update` applied: an omitted key is kept and an empty
    /// one removed.
    pub fn updated(&self, update: &ProviderKeysUpdate) -> Self {
        let apply = |saved: &Option<String>, change: Option<&str>| match change {
            None => saved.clone(),
            Some(change) => Some(change.trim().to_owned()).filter(|key| !key.is_empty()),
        };
        Self {
            llm_api_key: apply(&self.llm_api_key, update.llm_api_key()),
            huggingface_token: apply(&self.huggingface_token, update.huggingface_token()),
        }
    }

    /// A merge patch that replaces both saved keys, removing the absent ones.
    pub(crate) fn merge_patch(&self) -> serde_json::Value {
        serde_json::json!({
            "llm_api_key": self.llm_api_key,
            "huggingface_token": self.huggingface_token,
        })
    }
}

impl std::fmt::Debug for ProviderKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ProviderKeys").field(&self.status()).finish()
    }
}

/// The keys Studio uses now. A saved change reaches every database's next
/// connection and the relay's next request, without reopening projects.
pub struct ProviderKeyring {
    store: Arc<BackendStateStore>,
    keys: watch::Sender<ProviderKeys>,
    /// One save at a time, so the saved and the published keys agree.
    saving: Mutex<()>,
}

impl ProviderKeyring {
    pub fn new(store: Arc<BackendStateStore>, saved: ProviderKeys) -> Self {
        let (keys, _) = watch::channel(saved);
        Self {
            store,
            keys,
            saving: Mutex::new(()),
        }
    }

    pub fn current(&self) -> ProviderKeys {
        self.keys.borrow().clone()
    }

    /// Follow the keys, as a database does to open each connection with the
    /// current ones.
    pub fn subscribe(&self) -> watch::Receiver<ProviderKeys> {
        self.keys.subscribe()
    }

    pub async fn update(&self, update: &ProviderKeysUpdate) -> Result<ProviderKeys, String> {
        let _saving = self.saving.lock().await;
        let next = self.current().updated(update);
        self.store.save_keys(&next).await?;
        self.keys.send_replace(next.clone());
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_keep_omitted_keys_and_remove_empty_ones() {
        let saved = ProviderKeys::default().updated(
            &ProviderKeysUpdate::default()
                .with_llm_api_key(Some(" sk-or-first ".into()))
                .with_huggingface_token(Some("hf_first".into())),
        );
        assert_eq!(saved.llm_api_key(), Some("sk-or-first"));
        let kept = saved.updated(&ProviderKeysUpdate::default());
        assert_eq!(kept, saved);
        let removed =
            saved.updated(&ProviderKeysUpdate::default().with_huggingface_token(Some("  ".into())));
        assert_eq!(removed.huggingface_token(), None);
        assert_eq!(removed.llm_api_key(), Some("sk-or-first"));
        assert_eq!(removed.status(), ProviderKeysStatus::new(true, false));
        assert!(!format!("{saved:?}").contains("sk-or-first"));
    }
}
