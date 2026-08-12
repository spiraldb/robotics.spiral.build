use std::path::PathBuf;

/// Errors surfaced by artifact resolution, model loading, or inference.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid model artifact: {0}")]
    Artifact(String),
    #[error("invalid model configuration: {0}")]
    Configuration(String),
    #[error("invalid model checkpoint: {0}")]
    Checkpoint(String),
    #[error("invalid inference input: {0}")]
    Input(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[cfg(feature = "download")]
    #[error("could not download {url}: {source}")]
    Download {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait IoResultExt<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        let path = path.into();
        self.map_err(|source| Error::Io { path, source })
    }
}
