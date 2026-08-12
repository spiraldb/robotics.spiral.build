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
    repository: Option<String>,
    /// Immutable source commit, never a branch or tag.
    revision: Option<String>,
    config_sha256: Option<String>,
    weights_sha256: Option<String>,
}

impl ModelIdentity {
    pub fn local() -> Self {
        Self {
            repository: None,
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
            repository: None,
            revision: None,
            config_sha256: Some(config_sha256.to_ascii_lowercase()),
            weights_sha256: Some(weights_sha256.to_ascii_lowercase()),
        })
    }

    pub fn pinned(
        repository: impl Into<String>,
        revision: impl Into<String>,
        config_sha256: impl Into<String>,
        weights_sha256: impl Into<String>,
    ) -> Result<Self> {
        let repository = repository.into();
        let revision = revision.into();
        require_repository(&repository)?;
        require_hex(&revision, 40, "model revision")?;
        let mut identity = Self::content_addressed(config_sha256, weights_sha256)?;
        identity.repository = Some(repository);
        identity.revision = Some(revision.to_ascii_lowercase());
        Ok(identity)
    }

    pub fn repository(&self) -> Option<&str> {
        self.repository.as_deref()
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

    #[cfg(feature = "hub")]
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

#[cfg(feature = "hub")]
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

fn require_repository(repository: &str) -> Result<()> {
    let mut parts = repository.split('/');
    if parts.next().is_none_or(str::is_empty)
        || parts.next().is_none_or(str::is_empty)
        || parts.next().is_some()
    {
        return Err(Error::Artifact(format!(
            "Hugging Face model ID {repository:?} must be owner/name"
        )));
    }
    Ok(())
}

#[cfg(feature = "hub")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubModel {
    repository: String,
    revision: String,
    config_sha256: Option<String>,
    weights_sha256: Option<String>,
}

#[cfg(feature = "hub")]
impl HubModel {
    /// Construct a model pinned to an immutable 40-character Git commit.
    pub fn pinned(repository: impl Into<String>, revision: impl Into<String>) -> Result<Self> {
        let repository = repository.into();
        let revision = revision.into();
        require_repository(&repository)?;
        require_hex(&revision, 40, "model revision")?;
        Ok(Self {
            repository,
            revision: revision.to_ascii_lowercase(),
            config_sha256: None,
            weights_sha256: None,
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
        self.config_sha256 = Some(config_sha256.to_ascii_lowercase());
        self.weights_sha256 = Some(weights_sha256.to_ascii_lowercase());
        Ok(self)
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

#[cfg(feature = "hub")]
pub fn download_from_hub(model: &HubModel) -> Result<ModelFiles> {
    use hf_hub::{HFClientSync, split_id};

    let client = HFClientSync::new()?;
    let (owner, name) = split_id(&model.repository);
    let repository = client.model(owner, name);
    let config = repository
        .download_file()
        .filename(CONFIG_FILE)
        .revision(&model.revision)
        .send()?;
    let weights = repository
        .download_file()
        .filename(WEIGHTS_FILE)
        .revision(&model.revision)
        .send()?;
    let files = ModelFiles {
        config,
        weights,
        identity: ModelIdentity {
            repository: Some(model.repository.clone()),
            revision: Some(model.revision.clone()),
            config_sha256: model.config_sha256.clone(),
            weights_sha256: model.weights_sha256.clone(),
        },
    };
    files.verify_expected_files()?;
    Ok(files)
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

    #[cfg(feature = "hub")]
    #[test]
    fn mutable_hub_revisions_are_rejected() {
        assert!(super::HubModel::pinned("owner/model", "main").is_err());
        assert!(super::HubModel::pinned("owner/model", "a".repeat(40)).is_ok());
    }
}
