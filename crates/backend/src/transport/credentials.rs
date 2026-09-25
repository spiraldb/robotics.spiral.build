use spiral_robotics_common::providers::{LlmProvider, StudioProviders};
use spiral_robotics_storage::{ModelCredentials, SpiralConnectionSettings};
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use url::Url;

const LLM_BASE_URL: &str = "https://openrouter.ai/api/v1";
const LLM_MODEL: &str = "openai/gpt-5.6-terra";
const MAX_LOCAL_ENV_BYTES: u64 = 64 * 1024;
const HUGGING_FACE: &str = "https://huggingface.co";

/// The provider keys Studio holds. They never reach the browser: the relay
/// attaches each one to requests bound for its provider's origin.
pub(super) struct ProviderCredentials {
    /// A malformed local configuration only fails what needs the provider.
    llm: Result<Option<LlmCredentials>, String>,
    hugging_face: Option<String>,
}

impl ProviderCredentials {
    /// The LLM key comes from the environment or `.env.local`; the Hugging
    /// Face token from the saved connection.
    pub(super) fn load(saved: &SpiralConnectionSettings) -> Self {
        Self {
            llm: Self::configured_llm(),
            hugging_face: saved.huggingface_token().map(str::to_owned),
        }
    }

    /// The LLM provider as the environment and `.env.local` configure it now.
    fn configured_llm() -> Result<Option<LlmCredentials>, String> {
        Self::llm(|name| std::env::var(name).ok(), &Self::local_path())
    }

    fn local_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.local")
    }

    /// What the browser may know about the providers.
    pub(super) fn providers(&self) -> Result<StudioProviders, String> {
        let llm = self
            .llm
            .clone()?
            .map(|llm| LlmProvider::new(llm.base_url, llm.default_model));
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

    fn llm(
        mut lookup: impl FnMut(&str) -> Option<String>,
        local_path: &Path,
    ) -> Result<Option<LlmCredentials>, String> {
        let keys = ["LLM_API_KEY", "LLM_MODEL", "LLM_BASE_URL"];
        let mut values = BTreeMap::new();
        for key in keys {
            if let Some(value) = lookup(key).and_then(Self::non_empty) {
                values.insert(key.to_owned(), value);
            }
        }
        if values.len() < keys.len() {
            for (key, value) in Self::local_values(local_path)? {
                if keys.contains(&key.as_str()) {
                    values.entry(key).or_insert(value);
                }
            }
        }
        let Some(api_key) = values.remove("LLM_API_KEY") else {
            return Ok(None);
        };
        let base_url = values
            .remove("LLM_BASE_URL")
            .unwrap_or_else(|| LLM_BASE_URL.into());
        let parsed = Url::parse(&base_url)
            .map_err(|error| format!("Assistant provider endpoint is invalid: {error}"))?;
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return Err(
                "Assistant provider endpoint must use HTTPS (HTTP is allowed only for localhost)."
                    .into(),
            );
        }
        Ok(Some(LlmCredentials {
            base_url: parsed.as_str().trim_end_matches('/').into(),
            api_key,
            default_model: values
                .remove("LLM_MODEL")
                .unwrap_or_else(|| LLM_MODEL.into()),
        }))
    }

    fn non_empty(value: String) -> Option<String> {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    }

    fn local_values(path: &Path) -> Result<BTreeMap<String, String>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => {
                return Err(format!(
                    "Could not inspect the local Assistant credential file: {error}"
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("The local Assistant credential path must be a regular file.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(
                    "The local Assistant credential file must be readable only by its owner."
                        .into(),
                );
            }
        }
        if metadata.len() > MAX_LOCAL_ENV_BYTES {
            return Err("The local Assistant credential file exceeds its size bound.".into());
        }
        let bytes = fs::read(path).map_err(|error| {
            format!("Could not read the local Assistant credential file: {error}")
        })?;
        if bytes.len() > MAX_LOCAL_ENV_BYTES as usize {
            return Err("The local Assistant credential file exceeds its size bound.".into());
        }
        let mut values = BTreeMap::new();
        // Iterate without mutating process environment; parser errors may contain secrets.
        for entry in dotenvy::from_read_iter(bytes.as_slice()) {
            let (key, value) = entry.map_err(|_| {
                "The local Assistant credential file contains invalid dotenv syntax.".to_owned()
            })?;
            if let Some(value) = Self::non_empty(value) {
                values.insert(key, value);
            }
        }
        Ok(values)
    }
}

#[derive(Clone)]
struct LlmCredentials {
    base_url: String,
    api_key: String,
    default_model: String,
}

/// The LLM key as Spiral's model functions read it through the driver: the
/// provider is OpenRouter, so its key is the OpenRouter key. It is read again
/// for every new database connection, as the relay reads it for every
/// request, so a changed key reaches the driver without a restart. A
/// configuration that cannot be read gives the driver no key, and the model
/// functions say so.
pub(crate) struct DriverCredentials;

impl ModelCredentials for DriverCredentials {
    fn openrouter_api_key(&self) -> Option<String> {
        ProviderCredentials::configured_llm()
            .ok()
            .flatten()
            .map(|llm| llm.api_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct CredentialFile(tempfile::NamedTempFile);
    impl CredentialFile {
        fn new(contents: &str) -> Self {
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(contents.as_bytes()).unwrap();
            Self(file)
        }
        fn load(&self) -> Result<Option<LlmCredentials>, String> {
            ProviderCredentials::llm(|_| None, self.0.path())
        }
    }

    #[test]
    fn dotenv_credentials_preserve_defaults_quotes_and_process_precedence() {
        let file =
            CredentialFile::new("export LLM_API_KEY='local-secret' # comment\nUNRELATED=ignored\n");
        let local = file.load().unwrap().unwrap();
        assert_eq!(local.api_key, "local-secret");
        assert_eq!(local.base_url, LLM_BASE_URL);
        assert_eq!(local.default_model, LLM_MODEL);
        let process = BTreeMap::from([
            ("LLM_API_KEY", "process-secret"),
            ("LLM_MODEL", "process-model"),
        ]);
        let config = ProviderCredentials::llm(
            |name| process.get(name).map(ToString::to_string),
            file.0.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.api_key, "process-secret");
        assert_eq!(config.default_model, "process-model");
        assert_eq!(config.base_url, LLM_BASE_URL);
        assert!(
            CredentialFile::new("UNRELATED=secret\nLLM_API_KEY=  \n")
                .load()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn malformed_local_credentials_are_redacted_and_complete_process_config_skips_file() {
        let file = CredentialFile::new("LLM_API_KEY=\"never-log-this-secret\n");
        let error = file.load().err().expect("reject malformed credential");
        assert!(error.contains("invalid dotenv syntax"));
        assert!(!error.contains("never-log-this-secret"));
        let config = ProviderCredentials::llm(
            |name| {
                Some(
                    match name {
                        "LLM_API_KEY" => "process-secret",
                        "LLM_MODEL" => "process-model",
                        _ => "https://custom.example/v1",
                    }
                    .into(),
                )
            },
            file.0.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn keys_attach_only_to_their_providers_origin() {
        let credentials = ProviderCredentials {
            llm: Ok(Some(LlmCredentials {
                base_url: "https://openrouter.ai/api/v1".into(),
                api_key: "llm-key".into(),
                default_model: LLM_MODEL.into(),
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
            llm: Err("invalid dotenv syntax".into()),
            hugging_face: Some("hf-token".into()),
        };
        assert!(broken.providers().is_err());
        let url = Url::parse("https://huggingface.co/model").unwrap();
        assert_eq!(broken.bearer_for(&url), Some("hf-token"));
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_must_be_private_regular_and_bounded() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let file = CredentialFile::new("LLM_API_KEY=secret");
        file.0
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(file.load().err().unwrap().contains("only by its owner"));
        let directory = tempfile::tempdir().unwrap();
        let link = directory.path().join("credentials");
        symlink(file.0.path(), &link).unwrap();
        assert!(
            ProviderCredentials::local_values(&link)
                .unwrap_err()
                .contains("regular file")
        );
        file.0
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
        file.0.as_file().set_len(MAX_LOCAL_ENV_BYTES + 1).unwrap();
        assert!(file.load().err().unwrap().contains("size bound"));
    }
}
