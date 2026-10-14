//! Files of one Hugging Face repository at a pinned commit, read over HTTPS
//! with the deployment's Hugging Face token when it has one.
use super::{HuggingFaceRepository, HuggingFaceResponse as _};
use bytes::Bytes;
use url::Url;

pub(super) struct HuggingFaceDownloads {
    client: reqwest::Client,
    token: Option<String>,
    repository: HuggingFaceRepository,
    commit: String,
}

impl HuggingFaceDownloads {
    /// `repository`'s files at `commit`, read without a token.
    pub(super) fn new(repository: HuggingFaceRepository, commit: &str) -> Result<Self, String> {
        Ok(Self {
            client: HuggingFaceRepository::client()?,
            token: None,
            repository,
            commit: commit.to_owned(),
        })
    }

    /// The same files, read with `token` when there is one.
    pub(super) fn with_token(mut self, token: Option<&str>) -> Self {
        self.token = token.map(str::to_owned);
        self
    }

    /// Where `path` is read from.
    pub(super) fn url(&self, path: &str) -> Result<Url, String> {
        let mut url = Url::parse(&format!("{}/datasets/", super::HUGGING_FACE_ORIGIN))
            .map_err(|error| format!("building a Hugging Face download URL: {error}"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| "Hugging Face URL cannot accept path segments".to_owned())?;
            segments.pop_if_empty();
            segments
                .extend(self.repository.as_str().split('/'))
                .push("resolve")
                .push(&self.commit)
                .extend(path.split('/'));
        }
        Ok(url)
    }

    /// The token downloads authenticate with, if the deployment has one.
    fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// `path`'s pinned URI, as a Spiral Blob points at it.
    pub(super) fn pinned_uri(&self, path: &str) -> Result<String, String> {
        self.repository.file_uri(&self.commit, path)
    }

    /// The repository's root at its commit.
    pub(super) fn root_uri(&self) -> Result<String, String> {
        self.repository.file_uri(&self.commit, "")
    }

    /// The whole of `path`.
    pub(super) async fn bytes(&self, path: &str) -> Result<Bytes, String> {
        self.response(path)
            .await?
            .bytes()
            .await
            .map_err(|error| format!("downloading {path}: {error}"))
    }

    /// `path` written into `file` as it arrives, never held whole: a video
    /// file is gigabytes.
    pub(super) async fn save(&self, path: &str, file: &mut tokio::fs::File) -> Result<(), String> {
        use futures::StreamExt as _;
        use tokio::io::AsyncWriteExt as _;
        let failed = |error: &dyn std::fmt::Display| format!("downloading {path}: {error}");
        let mut body = self.response(path).await?.bytes_stream();
        while let Some(chunk) = body.next().await {
            file.write_all(&chunk.map_err(|error| failed(&error))?)
                .await
                .map_err(|error| failed(&error))?;
        }
        file.flush().await.map_err(|error| failed(&error))
    }

    async fn response(&self, path: &str) -> Result<reqwest::Response, String> {
        let mut request = self.client.get(self.url(path)?);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        request
            .send()
            .await
            .map_err(|error| format!("downloading {path}: {error}"))?
            .require_hugging_face_success(self.repository.as_str())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_at_the_pinned_commit() {
        let downloads = HuggingFaceDownloads::new(
            HuggingFaceRepository::try_from("lerobot/svla_so101_pickplace").unwrap(),
            "f641879e22172be7e8161d5e6c1503c2d2feb657",
        )
        .unwrap();
        assert_eq!(
            downloads
                .url("meta/episodes/chunk-000/file-000.parquet")
                .unwrap()
                .as_str(),
            "https://huggingface.co/datasets/lerobot/svla_so101_pickplace/resolve/f641879e22172be7e8161d5e6c1503c2d2feb657/meta/episodes/chunk-000/file-000.parquet"
        );
        assert_eq!(
            downloads.root_uri().unwrap(),
            "hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/"
        );
    }
}
