//! Files of one Hugging Face repository at a pinned commit, read over HTTPS
//! with the deployment's Hugging Face token when it has one, and the MCAP
//! files datasets point at by such pinned URIs.
use super::{HuggingFaceRepository, HuggingFaceResponse as _};
use crate::datasets::{DecodedEpisode, McapFile};
use bytes::Bytes;
use url::Url;

pub(super) struct HuggingFaceDownloads {
    client: reqwest::Client,
    token: Option<String>,
    origin: String,
    repository: HuggingFaceRepository,
    commit: String,
}

impl HuggingFaceDownloads {
    /// `repository`'s files at `commit`, read from Hugging Face without a
    /// token.
    pub(super) fn new(repository: HuggingFaceRepository, commit: &str) -> Result<Self, String> {
        Ok(Self {
            client: HuggingFaceRepository::client()?,
            token: None,
            origin: super::HUGGING_FACE_ORIGIN.to_owned(),
            repository,
            commit: commit.to_owned(),
        })
    }

    /// The same files, read with `token` when there is one.
    pub(super) fn with_token(mut self, token: Option<&str>) -> Self {
        self.token = token.map(str::to_owned);
        self
    }

    /// The same files, read from `origin` in place of Hugging Face's.
    fn with_origin(mut self, origin: &str) -> Self {
        self.origin = origin.to_owned();
        self
    }

    /// Where `path` is read from.
    pub(super) fn url(&self, path: &str) -> Result<Url, String> {
        let mut url = Url::parse(&format!("{}/datasets/", self.origin))
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

    /// `bytes`, downloaded from `path`, decoded as one episode whose file
    /// the dataset points at by its pinned URI.
    pub(super) async fn mcap_episode(
        &self,
        path: &str,
        bytes: Bytes,
    ) -> Result<DecodedEpisode, String> {
        let (path, uri) = (path.to_owned(), self.pinned_uri(path)?);
        tokio::task::spawn_blocking(move || {
            DecodedEpisode::of_mcap(McapFile::pinned(&path, uri, &bytes)?, &bytes)
        })
        .await
        .map_err(|error| format!("decoding an MCAP file failed: {error}"))?
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

/// The MCAP files datasets point at by their pinned Hugging Face URIs, as
/// the Hugging Face MCAP import keeps them: each file's key is its path in
/// the repository its URI pins.
pub(in crate::transfer) struct PinnedMcapFiles {
    origin: String,
    token: Option<String>,
}

impl PinnedMcapFiles {
    /// Read from Hugging Face with `token`, the deployment's, when there is
    /// one.
    pub(in crate::transfer) fn new(token: Option<&str>) -> Self {
        Self {
            origin: super::HUGGING_FACE_ORIGIN.to_owned(),
            token: token.map(str::to_owned),
        }
    }

    /// Read from a stand-in for Hugging Face at `origin`, without a token.
    #[cfg(test)]
    pub(in crate::transfer) fn at(origin: &str) -> Self {
        Self {
            origin: origin.to_owned(),
            token: None,
        }
    }

    /// The file a dataset keeps as `key` and points at by its pinned `uri`,
    /// downloaded at the commit `uri` names and decoded as one episode;
    /// refused unless it is `size` bytes, as the dataset recorded.
    pub(in crate::transfer) async fn episode(
        &self,
        key: &str,
        uri: &str,
        size: u64,
    ) -> Result<DecodedEpisode, String> {
        let downloads = self.downloads(key, uri)?;
        let bytes = downloads.bytes(key).await?;
        if bytes.len() as u64 != size {
            return Err(format!(
                "{uri} is {} bytes, not the {size} its dataset recorded",
                bytes.len()
            ));
        }
        downloads.mcap_episode(key, bytes).await
    }

    /// The files of the repository `uri` pins, at its commit; refused unless
    /// `uri` is `key`'s pinned URI.
    fn downloads(&self, key: &str, uri: &str) -> Result<HuggingFaceDownloads, String> {
        let not_pinned = || format!("{uri} is not the pinned Hugging Face URI of {key}");
        let commit = uri
            .split_once('@')
            .and_then(|(_, pinned)| pinned.split_once('/'))
            .map(|(commit, _)| commit)
            .ok_or_else(not_pinned)?;
        let repository = HuggingFaceRepository::try_from(uri).map_err(|_| not_pinned())?;
        let downloads = HuggingFaceDownloads::new(repository, commit)?
            .with_token(self.token.as_deref())
            .with_origin(&self.origin);
        if downloads.pinned_uri(key)? != uri {
            return Err(not_pinned());
        }
        Ok(downloads)
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

    #[test]
    fn a_pinned_file_is_read_where_its_uri_pins_it() {
        let pinned = PinnedMcapFiles::new(None);
        let uri = "hf://datasets/org/recordings@abcdef/episodes/take%20one.mcap";
        assert_eq!(
            pinned
                .downloads("episodes/take one.mcap", uri)
                .unwrap()
                .url("episodes/take one.mcap")
                .unwrap()
                .as_str(),
            "https://huggingface.co/datasets/org/recordings/resolve/abcdef/episodes/take%20one.mcap"
        );
        for (key, uri) in [
            ("episodes/other.mcap", uri),
            ("take.mcap", "hf://datasets/org/recordings/take.mcap"),
            (
                "take.mcap",
                "https://huggingface.co/datasets/org/recordings@abcdef/take.mcap",
            ),
        ] {
            let error = pinned.downloads(key, uri).err().unwrap();
            assert!(
                error.contains("is not the pinned Hugging Face URI"),
                "{error}"
            );
        }
    }
}
