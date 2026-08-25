use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::IoResultExt;
use crate::{Error, Result};

const CONFIG_FILE: &str = "config.json";
const WEIGHTS_FILE: &str = "model.safetensors";
const MAX_MODEL_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_MODEL_WEIGHTS_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ModelFileKind {
    Config,
    Weights,
}

impl ModelFileKind {
    #[cfg(feature = "download")]
    fn file_name(self) -> &'static str {
        match self {
            Self::Config => CONFIG_FILE,
            Self::Weights => WEIGHTS_FILE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Config => "model config",
            Self::Weights => "model weights",
        }
    }

    fn maximum_bytes(self) -> usize {
        match self {
            Self::Config => MAX_MODEL_CONFIG_BYTES,
            Self::Weights => MAX_MODEL_WEIGHTS_BYTES,
        }
    }

    fn validate_byte_len(self, byte_len: usize) -> Result<()> {
        let maximum_bytes = self.maximum_bytes();
        if byte_len > maximum_bytes {
            return Err(Error::Artifact(format!(
                "{} is {byte_len} bytes, exceeding the {maximum_bytes}-byte limit",
                self.label()
            )));
        }
        Ok(())
    }
}

struct ModelFile<'a> {
    path: &'a Path,
    kind: ModelFileKind,
}

impl<'a> ModelFile<'a> {
    fn new(path: &'a Path, kind: ModelFileKind) -> Self {
        Self { path, kind }
    }

    fn validate(&self) -> Result<()> {
        let metadata = std::fs::metadata(self.path).at(self.path)?;
        if !metadata.is_file() {
            return Err(Error::Artifact(format!(
                "{} is not a regular file",
                self.path.display()
            )));
        }
        let byte_len = usize::try_from(metadata.len()).map_err(|_| {
            Error::Artifact(format!(
                "{} size does not fit this process address space",
                self.kind.label()
            ))
        })?;
        self.kind.validate_byte_len(byte_len)
    }

    fn read(&self) -> Result<Vec<u8>> {
        use std::io::Read as _;

        let file = std::fs::File::open(self.path).at(self.path)?;
        let metadata = file.metadata().at(self.path)?;
        if !metadata.is_file() {
            return Err(Error::Artifact(format!(
                "{} is not a regular file",
                self.path.display()
            )));
        }
        let expected_byte_len = usize::try_from(metadata.len()).map_err(|_| {
            Error::Artifact(format!(
                "{} size does not fit this process address space",
                self.kind.label()
            ))
        })?;
        self.kind.validate_byte_len(expected_byte_len)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(expected_byte_len)
            .map_err(|error| {
                Error::Artifact(format!(
                    "could not reserve memory for {}: {error}",
                    self.kind.label()
                ))
            })?;
        file.take(
            u64::try_from(self.kind.maximum_bytes())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .at(self.path)?;
        self.kind.validate_byte_len(bytes.len())?;
        Ok(bytes)
    }

    #[cfg(feature = "download")]
    fn sha256(&self) -> Result<String> {
        use std::io::{BufReader, Read};

        let file = std::fs::File::open(self.path).at(self.path)?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let byte_count = reader.read(&mut buffer).at(self.path)?;
            if byte_count == 0 {
                break;
            }
            digest.update(&buffer[..byte_count]);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

/// Validated SHA-256 pair for a model's config and weights.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelDigests {
    config_sha256: String,
    weights_sha256: String,
}

impl ModelDigests {
    pub fn try_new(
        config_sha256: impl Into<String>,
        weights_sha256: impl Into<String>,
    ) -> Result<Self> {
        let config_sha256 = config_sha256.into();
        let weights_sha256 = weights_sha256.into();
        require_hex(&config_sha256, 64, "config SHA-256")?;
        require_hex(&weights_sha256, 64, "weights SHA-256")?;
        Ok(Self {
            config_sha256: config_sha256.to_ascii_lowercase(),
            weights_sha256: weights_sha256.to_ascii_lowercase(),
        })
    }

    pub fn config_sha256(&self) -> &str {
        &self.config_sha256
    }

    pub fn weights_sha256(&self) -> &str {
        &self.weights_sha256
    }
}

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
        let digests = ModelDigests::try_new(config_sha256, weights_sha256)?;
        Ok(Self {
            source: None,
            revision: None,
            config_sha256: Some(digests.config_sha256),
            weights_sha256: Some(digests.weights_sha256),
        })
    }

    pub fn pinned(
        source: impl Into<String>,
        revision: impl Into<String>,
        digests: ModelDigests,
    ) -> Result<Self> {
        let source = source.into();
        let revision = revision.into();
        require_source(&source)?;
        require_hex(&revision, 40, "model revision")?;
        Ok(Self {
            source: Some(source),
            revision: Some(revision.to_ascii_lowercase()),
            config_sha256: Some(digests.config_sha256),
            weights_sha256: Some(digests.weights_sha256),
        })
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
        ModelFile::new(&config, ModelFileKind::Config).validate()?;
        ModelFile::new(&weights, ModelFileKind::Weights).validate()?;
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
        let config = ModelFile::new(&self.config, ModelFileKind::Config).read()?;
        let weights = ModelFile::new(&self.weights, ModelFileKind::Weights).read()?;
        ArtifactProvenance::new(self.identity.clone(), self.weights.display().to_string())
            .bind(config, weights)
    }

    #[cfg(feature = "download")]
    fn verify_expected_files(&self) -> Result<()> {
        if let Some(expected) = self.identity.config_sha256.as_deref() {
            let actual = ModelFile::new(&self.config, ModelFileKind::Config).sha256()?;
            verify_digest("config", Some(expected), &actual)?;
        }
        if let Some(expected) = self.identity.weights_sha256.as_deref() {
            let actual = ModelFile::new(&self.weights, ModelFileKind::Weights).sha256()?;
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

struct ArtifactProvenance {
    identity: ModelIdentity,
    weights_label: String,
}

impl ArtifactProvenance {
    fn new(identity: ModelIdentity, weights_label: String) -> Self {
        Self {
            identity,
            weights_label,
        }
    }

    fn bind(mut self, config: Vec<u8>, weights: Vec<u8>) -> Result<ModelArtifact> {
        ModelFileKind::Config.validate_byte_len(config.len())?;
        ModelFileKind::Weights.validate_byte_len(weights.len())?;
        let config_sha256 = sha256_bytes(&config);
        let weights_sha256 = sha256_bytes(&weights);
        verify_digest(
            "config",
            self.identity.config_sha256.as_deref(),
            &config_sha256,
        )?;
        verify_digest(
            "weights",
            self.identity.weights_sha256.as_deref(),
            &weights_sha256,
        )?;
        self.identity.config_sha256 = Some(config_sha256);
        self.identity.weights_sha256 = Some(weights_sha256);
        Ok(ModelArtifact {
            config: config.into(),
            weights: weights.into(),
            weights_label: self.weights_label,
            identity: self.identity,
        })
    }
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
        ArtifactProvenance::new(identity, "in-memory model.safetensors".to_owned())
            .bind(config, weights)
    }

    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub fn config(&self) -> &[u8] {
        &self.config
    }

    pub fn weights(&self) -> &[u8] {
        &self.weights
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

/// Validated config and weights URLs for a remote model.
#[cfg(feature = "download")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteModelUrls {
    config: String,
    weights: String,
}

#[cfg(feature = "download")]
impl RemoteModelUrls {
    pub fn try_new(config: impl Into<String>, weights: impl Into<String>) -> Result<Self> {
        let config = config.into();
        let weights = weights.into();
        require_https_url(&config, "model config URL")?;
        require_https_url(&weights, "model weights URL")?;
        Ok(Self { config, weights })
    }

    pub fn config(&self) -> &str {
        &self.config
    }

    pub fn weights(&self) -> &str {
        &self.weights
    }
}

#[cfg(feature = "download")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteModel {
    urls: RemoteModelUrls,
    identity: ModelIdentity,
}

#[cfg(feature = "download")]
impl RemoteModel {
    /// Construct a raw-URL model pinned to an immutable 40-character source revision.
    pub fn pinned(
        source: impl Into<String>,
        revision: impl Into<String>,
        urls: RemoteModelUrls,
    ) -> Result<Self> {
        let source = source.into();
        let revision = revision.into();
        require_source(&source)?;
        require_hex(&revision, 40, "model revision")?;
        Ok(Self {
            urls,
            identity: ModelIdentity {
                source: Some(source),
                revision: Some(revision.to_ascii_lowercase()),
                config_sha256: None,
                weights_sha256: None,
            },
        })
    }

    pub fn with_digests(mut self, digests: ModelDigests) -> Self {
        self.identity.config_sha256 = Some(digests.config_sha256);
        self.identity.weights_sha256 = Some(digests.weights_sha256);
        self
    }

    pub fn config_url(&self) -> &str {
        self.urls.config()
    }

    pub fn weights_url(&self) -> &str {
        self.urls.weights()
    }

    pub fn download(&self) -> Result<ModelFiles> {
        let cache_root = std::env::var_os("SPIRAL_ROBOTICS_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::cache_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("spiral-robotics")
            })
            .join("models");
        self.download_to(cache_root)
    }

    pub fn download_to(&self, cache_root: impl AsRef<Path>) -> Result<ModelFiles> {
        let (Some(config_sha256), Some(weights_sha256)) = (
            self.identity.config_sha256.as_deref(),
            self.identity.weights_sha256.as_deref(),
        ) else {
            return Err(Error::Artifact(
                "remote models require both config and weights SHA-256 checksums before download"
                    .to_owned(),
            ));
        };
        let cache_key = sha256_bytes(
            format!(
                "{}\n{}\n{}\n{}",
                self.identity.source.as_deref().unwrap_or_default(),
                self.identity.revision.as_deref().unwrap_or_default(),
                self.urls.config,
                self.urls.weights,
            )
            .as_bytes(),
        );
        let directory = cache_root.as_ref().join(cache_key);
        std::fs::create_dir_all(&directory).at(&directory)?;
        let config = directory.join(ModelFileKind::Config.file_name());
        let weights = directory.join(ModelFileKind::Weights.file_name());
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("spiral-robotics/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|source| Error::Download {
                url: "HTTP client".to_owned(),
                source,
            })?;
        DownloadTarget::new(&self.urls.config, &config, ModelFileKind::Config)
            .with_expected_sha256(config_sha256)
            .download(&client)?;
        DownloadTarget::new(&self.urls.weights, &weights, ModelFileKind::Weights)
            .with_expected_sha256(weights_sha256)
            .download(&client)?;
        let files = ModelFiles {
            config,
            weights,
            identity: self.identity.clone(),
        };
        files.verify_expected_files()?;
        Ok(files)
    }
}

#[cfg(feature = "download")]
struct DownloadTarget<'a> {
    url: &'a str,
    destination: &'a Path,
    expected_sha256: Option<&'a str>,
    kind: ModelFileKind,
}

#[cfg(feature = "download")]
impl<'a> DownloadTarget<'a> {
    fn new(url: &'a str, destination: &'a Path, kind: ModelFileKind) -> Self {
        Self {
            url,
            destination,
            expected_sha256: None,
            kind,
        }
    }

    fn with_expected_sha256(mut self, expected_sha256: &'a str) -> Self {
        self.expected_sha256 = Some(expected_sha256);
        self
    }

    fn download(&self, client: &reqwest::blocking::Client) -> Result<()> {
        use std::io::Write as _;

        if self.destination.is_file() {
            let expected = self.expected_sha256.ok_or_else(|| {
                Error::Artifact(format!(
                    "refusing to reuse checksum-less cached artifact {}",
                    self.destination.display()
                ))
            })?;
            let cached_file = ModelFile::new(self.destination, self.kind);
            if cached_file.validate().is_ok()
                && cached_file
                    .sha256()
                    .is_ok_and(|actual| actual.eq_ignore_ascii_case(expected))
            {
                return Ok(());
            }
        }

        let mut response = client
            .get(self.url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|source| Error::Download {
                url: self.url.to_owned(),
                source,
            })?;
        let parent = self
            .destination
            .parent()
            .ok_or_else(|| Error::Artifact("download destination has no parent".to_owned()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).at(parent)?;
        let maximum_bytes = self.kind.maximum_bytes();
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(maximum_bytes).unwrap_or(u64::MAX))
        {
            return Err(Error::Artifact(format!(
                "download from {} exceeds the {maximum_bytes}-byte artifact limit",
                self.url
            )));
        }
        self.copy_with_limit(&mut response, temporary.as_file_mut())
            .at(self.destination)?;
        temporary.flush().at(self.destination)?;
        if let Some(expected) = self.expected_sha256 {
            let actual = ModelFile::new(temporary.path(), self.kind).sha256()?;
            verify_digest("download", Some(expected), &actual)?;
        }
        temporary
            .persist(self.destination)
            .map_err(|error| Error::Io {
                path: self.destination.to_owned(),
                source: error.error,
            })?;
        Ok(())
    }

    fn copy_with_limit(
        &self,
        reader: &mut impl std::io::Read,
        writer: &mut impl std::io::Write,
    ) -> std::io::Result<u64> {
        let maximum_bytes = self.kind.maximum_bytes();
        let limit = u64::try_from(maximum_bytes).unwrap_or(u64::MAX);
        let mut limited = std::io::Read::take(reader, limit.saturating_add(1));
        let copied = std::io::copy(&mut limited, writer)?;
        if copied > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("artifact exceeds the {maximum_bytes}-byte limit"),
            ));
        }
        Ok(copied)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_MODEL_CONFIG_BYTES, ModelFileKind, require_hex, verify_digest};

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

    #[test]
    fn model_byte_limits_are_inclusive() {
        assert!(
            ModelFileKind::Config
                .validate_byte_len(MAX_MODEL_CONFIG_BYTES)
                .is_ok()
        );
        assert!(
            ModelFileKind::Config
                .validate_byte_len(MAX_MODEL_CONFIG_BYTES + 1)
                .is_err()
        );
        assert!(
            ModelFileKind::Weights
                .validate_byte_len(super::MAX_MODEL_WEIGHTS_BYTES)
                .is_ok()
        );
    }

    #[test]
    fn oversized_local_files_are_rejected_before_reading() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join(super::CONFIG_FILE);
        let weights = directory.path().join(super::WEIGHTS_FILE);
        std::fs::File::create(&config)
            .unwrap()
            .set_len((MAX_MODEL_CONFIG_BYTES + 1) as u64)
            .unwrap();
        std::fs::write(weights, b"weights").unwrap();
        let error = super::ModelFiles::from_directory(directory.path()).unwrap_err();
        assert!(error.to_string().contains("model config"));
    }

    #[cfg(feature = "download")]
    #[test]
    fn remote_models_require_pinned_revisions_and_https_urls() {
        let valid_urls = super::RemoteModelUrls::try_new(
            "https://example.com/config.json",
            "https://example.com/model.safetensors",
        )
        .unwrap();
        assert!(super::RemoteModel::pinned("model source", "main", valid_urls,).is_err());
        assert!(
            super::RemoteModelUrls::try_new(
                "http://example.com/config.json",
                "https://example.com/model.safetensors",
            )
            .is_err()
        );
    }

    #[cfg(feature = "download")]
    #[test]
    fn checksum_less_cache_entries_are_never_trusted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"substituted").unwrap();
        let client = reqwest::blocking::Client::new();
        let error = super::DownloadTarget::new(
            "https://example.invalid/config.json",
            &path,
            ModelFileKind::Config,
        )
        .download(&client)
        .unwrap_err();
        assert!(error.to_string().contains("checksum-less cached artifact"));
    }

    #[cfg(feature = "download")]
    #[test]
    fn streaming_download_limit_stops_at_one_byte_over_the_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let target = super::DownloadTarget::new(
            "https://example.invalid/config.json",
            &path,
            ModelFileKind::Config,
        );
        let mut exact = std::io::Cursor::new(vec![0_u8; MAX_MODEL_CONFIG_BYTES]);
        let mut output = Vec::new();
        assert_eq!(
            target.copy_with_limit(&mut exact, &mut output).unwrap(),
            MAX_MODEL_CONFIG_BYTES as u64
        );

        let mut oversized = std::io::Cursor::new(vec![0_u8; MAX_MODEL_CONFIG_BYTES + 1]);
        let mut output = Vec::new();
        assert!(target.copy_with_limit(&mut oversized, &mut output).is_err());
        assert_eq!(output.len(), MAX_MODEL_CONFIG_BYTES + 1);
    }

    #[cfg(feature = "download")]
    #[test]
    fn remote_download_requires_the_checksum_pair_before_network_or_cache_io() {
        let urls = super::RemoteModelUrls::try_new(
            "https://example.invalid/config.json",
            "https://example.invalid/model.safetensors",
        )
        .unwrap();
        let model = super::RemoteModel::pinned("model source", "a".repeat(40), urls).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let error = model.download_to(directory.path()).unwrap_err();
        assert!(error.to_string().contains("both config and weights"));
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none()
        );
    }
}
