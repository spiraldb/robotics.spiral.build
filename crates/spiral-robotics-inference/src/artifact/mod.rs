use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::IoResultExt;
use crate::{Error, Result};

pub const CONFIG_FILE: &str = "config.json";
pub const WEIGHTS_FILE: &str = "model.safetensors";

/// Content-addressed identity of the exact bytes admitted to the runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelIdentity {
    source: Option<String>,
    /// Immutable source commit, never a branch or tag.
    revision: Option<String>,
    config_sha256: Option<String>,
    weights_sha256: Option<String>,
}

impl ModelIdentity {
    pub fn local() -> Self {
        Self {
            source: None,
            revision: None,
            config_sha256: None,
            weights_sha256: None,
        }
    }

    pub fn content_addressed(
        config_sha256: impl Into<String>,
        weights_sha256: impl Into<String>,
    ) -> Result<Self> {
        let config_sha256 = config_sha256.into();
        let weights_sha256 = weights_sha256.into();
        require_hex(&config_sha256, 64, "config SHA-256")?;
        require_hex(&weights_sha256, 64, "weights SHA-256")?;
        Ok(Self {
            source: None,
            revision: None,
            config_sha256: Some(config_sha256.to_ascii_lowercase()),
            weights_sha256: Some(weights_sha256.to_ascii_lowercase()),
        })
    }

    pub fn pinned(
        source: impl Into<String>,
        revision: impl Into<String>,
        config_sha256: impl Into<String>,
        weights_sha256: impl Into<String>,
    ) -> Result<Self> {
        let source = source.into();
        let revision = revision.into();
        require_source(&source)?;
        require_hex(&revision, 40, "model revision")?;
        let mut identity = Self::content_addressed(config_sha256, weights_sha256)?;
        identity.source = Some(source);
        identity.revision = Some(revision.to_ascii_lowercase());
        Ok(identity)
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    pub fn config_sha256(&self) -> Option<&str> {
        self.config_sha256.as_deref()
    }

    pub fn weights_sha256(&self) -> Option<&str> {
        self.weights_sha256.as_deref()
    }
}

/// Paths and expected identity for one model artifact pair.
#[derive(Clone, Debug)]
pub struct ModelFiles {
    config: PathBuf,
    weights: PathBuf,
    identity: ModelIdentity,
}

impl ModelFiles {
    pub fn from_directory(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_directory_with_identity(path, ModelIdentity::local())
    }

    pub fn from_directory_with_identity(
        path: impl AsRef<Path>,
        identity: ModelIdentity,
    ) -> Result<Self> {
        let path = path.as_ref();
        let config = path.join(CONFIG_FILE);
        let weights = path.join(WEIGHTS_FILE);
        require_file(&config)?;
        require_file(&weights)?;
        Ok(Self {
            config,
            weights,
            identity,
        })
    }

    pub fn config_path(&self) -> &Path {
        &self.config
    }

    pub fn weights_path(&self) -> &Path {
        &self.weights
    }

    /// Expected source/content identity. The loaded `ModelArtifact` always
    /// replaces absent hashes with digests of the bytes it actually owns.
    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    /// Copy both files into process-owned memory, verify any expected hashes,
    /// and bind the identity to the bytes that will actually be parsed.
    pub fn load(&self) -> Result<ModelArtifact> {
        let config = std::fs::read(&self.config).at(&self.config)?;
        let weights = std::fs::read(&self.weights).at(&self.weights)?;
        ModelArtifact::from_bytes_with_identity_and_label(
            config,
            weights,
            self.identity.clone(),
            self.weights.display().to_string(),
        )
    }

    #[cfg(feature = "download")]
    fn verify_expected_files(&self) -> Result<()> {
        if let Some(expected) = self.identity.config_sha256.as_deref() {
            let actual = sha256_file(&self.config)?;
            verify_digest("config", Some(expected), &actual)?;
        }
        if let Some(expected) = self.identity.weights_sha256.as_deref() {
            let actual = sha256_file(&self.weights)?;
            verify_digest("weights", Some(expected), &actual)?;
        }
        Ok(())
    }
}

/// Owned, content-addressed model bytes. Artifact sources converge on this
/// type before policy-specific parsing or backend materialization begins.
#[derive(Clone)]
pub struct ModelArtifact {
    config: Arc<[u8]>,
    weights: Arc<[u8]>,
    weights_label: String,
    identity: ModelIdentity,
}

impl std::fmt::Debug for ModelArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelArtifact")
            .field("config_bytes", &self.config.len())
            .field("weights_bytes", &self.weights.len())
            .field("weights_label", &self.weights_label)
            .field("identity", &self.identity)
            .finish()
    }
}

impl ModelArtifact {
    pub fn from_bytes(config: Vec<u8>, weights: Vec<u8>) -> Result<Self> {
        Self::from_bytes_with_identity(config, weights, ModelIdentity::local())
    }

    pub fn from_bytes_with_identity(
        config: Vec<u8>,
        weights: Vec<u8>,
        identity: ModelIdentity,
    ) -> Result<Self> {
        Self::from_bytes_with_identity_and_label(
            config,
            weights,
            identity,
            "in-memory model.safetensors".to_owned(),
        )
    }

    fn from_bytes_with_identity_and_label(
        config: Vec<u8>,
        weights: Vec<u8>,
        mut identity: ModelIdentity,
        weights_label: String,
    ) -> Result<Self> {
        let config_sha256 = sha256_bytes(&config);
        let weights_sha256 = sha256_bytes(&weights);
        verify_digest("config", identity.config_sha256.as_deref(), &config_sha256)?;
        verify_digest(
            "weights",
            identity.weights_sha256.as_deref(),
            &weights_sha256,
        )?;
        identity.config_sha256 = Some(config_sha256);
        identity.weights_sha256 = Some(weights_sha256);
        Ok(Self {
            config: config.into(),
            weights: weights.into(),
            weights_label,
            identity,
        })
    }

    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub(crate) fn into_parts(self) -> (Arc<[u8]>, Arc<[u8]>, String, ModelIdentity) {
        (self.config, self.weights, self.weights_label, self.identity)
    }
}

fn require_file(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).at(path)?;
    if !metadata.is_file() {
        return Err(Error::Artifact(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(feature = "download")]
fn sha256_file(path: &Path) -> Result<String> {
    use std::io::{BufReader, Read};

    let file = std::fs::File::open(path).at(path)?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).at(path)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_digest(label: &str, expected: Option<&str>, actual: &str) -> Result<()> {
    if let Some(expected) = expected
        && !expected.eq_ignore_ascii_case(actual)
    {
        return Err(Error::Artifact(format!(
            "{label} SHA-256 mismatch: expected {expected}, found {actual}"
        )));
    }
    Ok(())
}

fn require_hex(value: &str, length: usize, label: &str) -> Result<()> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Artifact(format!(
            "{label} must be exactly {length} hexadecimal characters"
        )));
    }
    Ok(())
}

fn require_source(source: &str) -> Result<()> {
    if source.trim().is_empty() {
        return Err(Error::Artifact("model source cannot be empty".to_owned()));
    }
    Ok(())
}

#[cfg(feature = "download")]
fn require_https_url(url: &str, label: &str) -> Result<()> {
    if !url.starts_with("https://") || url.len() == "https://".len() {
        return Err(Error::Artifact(format!(
            "{label} must be an absolute HTTPS URL"
        )));
    }
    Ok(())
}

#[cfg(feature = "download")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteModel {
    config_url: String,
    weights_url: String,
    identity: ModelIdentity,
}

#[cfg(feature = "download")]
impl RemoteModel {
    /// Construct a raw-URL model pinned to an immutable 40-character source revision.
    pub fn pinned(
        source: impl Into<String>,
        revision: impl Into<String>,
        config_url: impl Into<String>,
        weights_url: impl Into<String>,
    ) -> Result<Self> {
        let source = source.into();
        let revision = revision.into();
        let config_url = config_url.into();
        let weights_url = weights_url.into();
        require_source(&source)?;
        require_hex(&revision, 40, "model revision")?;
        require_https_url(&config_url, "model config URL")?;
        require_https_url(&weights_url, "model weights URL")?;
        Ok(Self {
            config_url,
            weights_url,
            identity: ModelIdentity {
                source: Some(source),
                revision: Some(revision.to_ascii_lowercase()),
                config_sha256: None,
                weights_sha256: None,
            },
        })
    }

    pub fn with_checksums(
        mut self,
        config_sha256: impl Into<String>,
        weights_sha256: impl Into<String>,
    ) -> Result<Self> {
        let config_sha256 = config_sha256.into();
        let weights_sha256 = weights_sha256.into();
        require_hex(&config_sha256, 64, "config SHA-256")?;
        require_hex(&weights_sha256, 64, "weights SHA-256")?;
        self.identity.config_sha256 = Some(config_sha256.to_ascii_lowercase());
        self.identity.weights_sha256 = Some(weights_sha256.to_ascii_lowercase());
        Ok(self)
    }

    pub fn config_url(&self) -> &str {
        &self.config_url
    }

    pub fn weights_url(&self) -> &str {
        &self.weights_url
    }
}

#[cfg(feature = "download")]
pub fn download_remote_model(model: &RemoteModel) -> Result<ModelFiles> {
    let cache = std::env::var_os("SPIRAL_ROBOTICS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("spiral-robotics")
        })
        .join("models");
    download_remote_model_to(model, cache)
}

#[cfg(feature = "download")]
pub fn download_remote_model_to(
    model: &RemoteModel,
    cache_root: impl AsRef<Path>,
) -> Result<ModelFiles> {
    let cache_key = sha256_bytes(
        format!(
            "{}\n{}\n{}\n{}",
            model.identity.source.as_deref().unwrap_or_default(),
            model.identity.revision.as_deref().unwrap_or_default(),
            model.config_url,
            model.weights_url,
        )
        .as_bytes(),
    );
    let directory = cache_root.as_ref().join(cache_key);
    std::fs::create_dir_all(&directory).at(&directory)?;
    let config = directory.join(CONFIG_FILE);
    let weights = directory.join(WEIGHTS_FILE);
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("spiral-robotics/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|source| Error::Download {
            url: "HTTP client".to_owned(),
            source,
        })?;
    download_file(
        &client,
        &model.config_url,
        &config,
        model.identity.config_sha256.as_deref(),
    )?;
    download_file(
        &client,
        &model.weights_url,
        &weights,
        model.identity.weights_sha256.as_deref(),
    )?;
    let files = ModelFiles {
        config,
        weights,
        identity: model.identity.clone(),
    };
    files.verify_expected_files()?;
    Ok(files)
}

#[cfg(feature = "download")]
fn download_file(
    client: &reqwest::blocking::Client,
    url: &str,
    destination: &Path,
    expected_sha256: Option<&str>,
) -> Result<()> {
    use std::io::Write as _;

    if destination.is_file()
        && expected_sha256
            .map(|expected| {
                sha256_file(destination).is_ok_and(|actual| actual.eq_ignore_ascii_case(expected))
            })
            .unwrap_or(true)
    {
        return Ok(());
    }

    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|source| Error::Download {
            url: url.to_owned(),
            source,
        })?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Artifact("download destination has no parent".to_owned()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).at(parent)?;
    std::io::copy(&mut response, temporary.as_file_mut()).at(destination)?;
    temporary.flush().at(destination)?;
    if let Some(expected) = expected_sha256 {
        let actual = sha256_file(temporary.path())?;
        verify_digest("download", Some(expected), &actual)?;
    }
    temporary.persist(destination).map_err(|error| Error::Io {
        path: destination.to_owned(),
        source: error.error,
    })?;
    Ok(())
}

#[cfg(feature = "download")]
pub(crate) fn download_verified_file(url: &str, destination: &Path, sha256: &str) -> Result<()> {
    require_https_url(url, "artifact URL")?;
    require_hex(sha256, 64, "artifact SHA-256")?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Artifact("download destination has no parent".to_owned()))?;
    std::fs::create_dir_all(parent).at(parent)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("spiral-robotics/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|source| Error::Download {
            url: "HTTP client".to_owned(),
            source,
        })?;
    download_file(&client, url, destination, Some(sha256))
}

#[cfg(test)]
mod tests {
    use super::{require_hex, verify_digest};

    #[test]
    fn immutable_identifiers_have_exact_lengths() {
        assert!(require_hex(&"a".repeat(40), 40, "revision").is_ok());
        assert!(require_hex("main", 40, "revision").is_err());
        assert!(require_hex(&"g".repeat(40), 40, "revision").is_err());
    }

    #[test]
    fn expected_content_digest_is_enforced() {
        assert!(verify_digest("weights", Some("abcd"), "abcd").is_ok());
        assert!(verify_digest("weights", Some("abcd"), "dcba").is_err());
    }

    #[test]
    fn in_memory_artifacts_receive_the_identity_of_their_actual_bytes() {
        let artifact = super::ModelArtifact::from_bytes(b"{}".to_vec(), b"weights".to_vec())
            .expect("owned bytes should be content addressed");
        assert_eq!(artifact.identity().config_sha256().unwrap().len(), 64);
        assert_eq!(artifact.identity().weights_sha256().unwrap().len(), 64);
    }

    #[cfg(feature = "download")]
    #[test]
    fn remote_models_require_pinned_revisions_and_https_urls() {
        assert!(
            super::RemoteModel::pinned(
                "model source",
                "main",
                "https://example.com/config.json",
                "https://example.com/model.safetensors",
            )
            .is_err()
        );
        assert!(
            super::RemoteModel::pinned(
                "model source",
                "a".repeat(40),
                "http://example.com/config.json",
                "https://example.com/model.safetensors",
            )
            .is_err()
        );
    }
}
