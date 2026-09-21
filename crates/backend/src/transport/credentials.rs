use spiral_robotics_common::credentials::{
    HuggingFaceCredentials, LlmCredentials, SpiralCredentials, StudioCredentials,
};
use spiral_robotics_storage::BackendStateStore;
use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};
use url::Url;

const LLM_BASE_URL: &str = "https://openrouter.ai/api/v1";
const LLM_MODEL: &str = "openai/gpt-5.6-terra";
const MAX_LOCAL_ENV_BYTES: u64 = 64 * 1024;

pub(super) struct ConfiguredCredentials;

impl ConfiguredCredentials {
    pub(super) fn load(state: &BackendStateStore) -> Result<StudioCredentials, String> {
        let local_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.local");
        let llm = Self::llm(|name| std::env::var(name).ok(), &local_path)?;
        let spiral = state.credentials().map(|credentials| {
            SpiralCredentials::new(
                state.spiral_endpoint().unwrap_or_else(|| {
                    spiral_robotics_storage::ServiceEndpoint::environment_spiral().into_string()
                }),
                state.spiral_spfs_endpoint().unwrap_or_else(|| {
                    spiral_robotics_storage::ServiceEndpoint::environment_spiral_spfs()
                        .into_string()
                }),
                credentials.client_id().to_owned(),
                credentials.client_secret().to_owned(),
            )
        });
        let hugging_face = state.huggingface_token().map(HuggingFaceCredentials::new);
        Ok(StudioCredentials::new(llm, spiral, hugging_face))
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
        Ok(Some(LlmCredentials::new(
            parsed.as_str().trim_end_matches('/').into(),
            api_key,
            values
                .remove("LLM_MODEL")
                .unwrap_or_else(|| LLM_MODEL.into()),
        )))
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
            ConfiguredCredentials::llm(|_| None, self.0.path())
        }
    }

    #[test]
    fn dotenv_credentials_preserve_defaults_quotes_and_process_precedence() {
        let file =
            CredentialFile::new("export LLM_API_KEY='local-secret' # comment\nUNRELATED=ignored\n");
        let local = file.load().unwrap().unwrap();
        assert_eq!(local.api_key(), "local-secret");
        assert_eq!(local.base_url(), LLM_BASE_URL);
        assert_eq!(local.default_model(), LLM_MODEL);
        let process = BTreeMap::from([
            ("LLM_API_KEY", "process-secret"),
            ("LLM_MODEL", "process-model"),
        ]);
        let config = ConfiguredCredentials::llm(
            |name| process.get(name).map(ToString::to_string),
            file.0.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.api_key(), "process-secret");
        assert_eq!(config.default_model(), "process-model");
        assert_eq!(config.base_url(), LLM_BASE_URL);
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
        let config = ConfiguredCredentials::llm(
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
        assert_eq!(config.base_url(), "https://custom.example/v1");
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
            ConfiguredCredentials::local_values(&link)
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
