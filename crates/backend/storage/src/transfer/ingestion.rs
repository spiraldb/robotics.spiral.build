//! Hugging Face repositories: discovery, which resolves a repository's
//! revision to a commit and lists its files without changing Spiral, and
//! import, which downloads and decodes every episode at that commit and
//! writes them into a new dataset 32 at a time
//! ([datasets](../../../../../docs/datasets.md#writing)).
mod downloads;
mod lerobot;

use downloads::HuggingFaceDownloads;
pub(super) use downloads::PinnedMcapFiles;
use lerobot::LeRobotRepository;
use reqwest::StatusCode;
use reqwest::header::LINK;
use serde::Deserialize;
use spiral_robotics_common::datasets::{
    DatasetFormat, DatasetImportPhase, DatasetSelection, HuggingFaceDatasetFile,
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource, HuggingFaceInspectionContents,
    HuggingFaceInspectionSource,
};
use url::Url;

use crate::datasets::{DatasetImport, DatasetWrite, DecodedEpisode};
use crate::manager::StorageManager;

const HUGGING_FACE_ORIGIN: &str = "https://huggingface.co";
/// The most entries a listed repository may hold. It also bounds the listing:
/// Hugging Face answers about 1,000 entries a page, and a page that adds none
/// ends it.
const MAX_REPOSITORY_FILES: usize = 50_000;
#[derive(Debug, Deserialize)]
struct RepositoryInfo {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HuggingFaceRepository(String);

impl TryFrom<&str> for HuggingFaceRepository {
    type Error = String;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        let trimmed = input.trim().trim_end_matches('/');
        let (candidate, allow_suffix) = if let Some(value) = trimmed.strip_prefix("hf://datasets/")
        {
            (value.split('@').next().unwrap_or(value).to_owned(), false)
        } else if let Ok(url) = Url::parse(trimmed) {
            if url.scheme() != "https" || url.host_str() != Some("huggingface.co") {
                return Err("use a huggingface.co dataset URL or namespace/repository".to_owned());
            }
            (
                url.path()
                    .trim_start_matches('/')
                    .strip_prefix("datasets/")
                    .ok_or_else(|| "the Hugging Face URL must point to /datasets/...".to_owned())?
                    .to_owned(),
                true,
            )
        } else {
            (trimmed.to_owned(), false)
        };
        let segments = candidate
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        if segments.len() < 2
            || (!allow_suffix && segments.len() != 2)
            || segments.iter().any(|segment| {
                segment.len() > 96
                    || !segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
                    })
            })
        {
            return Err(
                "repository must be namespace/name using Hugging Face-safe characters".to_owned(),
            );
        }
        Ok(Self(format!("{}/{}", segments[0], segments[1])))
    }
}

impl HuggingFaceRepository {
    fn as_str(&self) -> &str {
        &self.0
    }

    /// The client every request to Hugging Face is sent with.
    fn client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(concat!(
                "spiral-robotics-studio/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| format!("creating the Hugging Face client: {error}"))
    }

    fn api_url(&self, operation: &str, revision: &str) -> Result<Url, String> {
        let mut url = Url::parse(&format!("{HUGGING_FACE_ORIGIN}/api/datasets/"))
            .map_err(|error| format!("building the Hugging Face API URL: {error}"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| "Hugging Face API URL cannot accept path segments".to_owned())?;
            segments.pop_if_empty();
            for segment in self.0.split('/') {
                segments.push(segment);
            }
            segments.push(operation).push(revision);
        }
        Ok(url)
    }

    async fn tree(
        &self,
        client: &reqwest::Client,
        revision: &str,
    ) -> Result<Vec<TreeEntry>, String> {
        let mut first_page = self.api_url("tree", revision)?;
        // Without `expand`: it adds each file's last commit, which nothing reads,
        // and cuts pages to 50 entries.
        first_page
            .query_pairs_mut()
            .append_pair("recursive", "true");
        let mut next = Some(first_page);
        let mut files = Vec::new();
        while let Some(url) = next.take() {
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|error| format!("listing Hugging Face repository files: {error}"))?
                .require_hugging_face_success(self.as_str())
                .await?;
            let link = response
                .headers()
                .get(LINK)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let mut page = response
                .json::<Vec<TreeEntry>>()
                .await
                .map_err(|error| format!("decoding the Hugging Face file tree: {error}"))?;
            if page.is_empty() && link.is_some() {
                return Err("Hugging Face returned an empty page of the file listing".to_owned());
            }
            files.append(&mut page);
            if files.len() > MAX_REPOSITORY_FILES {
                return Err(format!(
                    "repository contains more than {MAX_REPOSITORY_FILES} entries; narrow the source before importing"
                ));
            }
            next = link
                .as_deref()
                .map(Self::parse_next_link)
                .transpose()?
                .flatten();
        }
        Ok(files)
    }

    fn parse_next_link(value: &str) -> Result<Option<Url>, String> {
        let Some(link) = value
            .split(',')
            .find(|link| link.contains("rel=\"next\"") || link.contains("rel=next"))
        else {
            return Ok(None);
        };
        let start = link
            .find('<')
            .ok_or_else(|| "Hugging Face pagination link has no URL".to_owned())?;
        let end = link[start + 1..]
            .find('>')
            .map(|offset| start + 1 + offset)
            .ok_or_else(|| "Hugging Face pagination link is malformed".to_owned())?;
        let url = Url::parse(&link[start + 1..end])
            .map_err(|error| format!("parsing Hugging Face pagination link: {error}"))?;
        if url.scheme() != "https" || url.host_str() != Some("huggingface.co") {
            return Err("Hugging Face returned an unexpected pagination origin".to_owned());
        }
        Ok(Some(url))
    }

    /// `path` at `revision`, as a Spiral Blob points at it; the repository's
    /// root for an empty `path`.
    fn file_uri(&self, revision: &str, path: &str) -> Result<String, String> {
        let mut url = Url::parse("hf://datasets/")
            .map_err(|error| format!("building a Spiral Hugging Face URI: {error}"))?;
        let mut repository_parts = self.0.split('/');
        let owner = repository_parts
            .next()
            .ok_or_else(|| "Hugging Face repository has no owner".to_owned())?;
        let name = repository_parts
            .next()
            .ok_or_else(|| "Hugging Face repository has no name".to_owned())?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| "Spiral Hugging Face URI cannot accept path segments".to_owned())?;
            segments.pop_if_empty();
            segments.push(owner).push(&format!("{name}@{revision}"));
            for segment in path.split('/') {
                segments.push(segment);
            }
        }
        Ok(url.to_string())
    }
}

#[allow(async_fn_in_trait)]
trait HuggingFaceResponse {
    async fn require_hugging_face_success(
        self,
        repository: &str,
    ) -> Result<reqwest::Response, String>;
}

impl HuggingFaceResponse for reqwest::Response {
    async fn require_hugging_face_success(
        self,
        repository: &str,
    ) -> Result<reqwest::Response, String> {
        let status = self.status();
        if status.is_success() {
            return Ok(self);
        }
        let detail = self.text().await.unwrap_or_default();
        let hint = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "Studio currently imports public Hugging Face repositories; this source requires authorization"
            }
            StatusCode::NOT_FOUND => "the repository or revision was not found",
            _ => "Hugging Face refused the request",
        };
        let detail = detail.trim();
        if detail.is_empty() {
            Err(format!("{hint}: {repository} ({status})"))
        } else {
            Err(format!("{hint}: {repository} ({status}: {detail})"))
        }
    }
}

/// Storage-backed discovery behavior for the API inspection result type.
#[allow(async_fn_in_trait)]
pub trait HuggingFaceDatasetDiscovery: Sized {
    /// Resolve one public Hugging Face repository of MCAP recordings without
    /// changing Spiral.
    async fn discover(request: HuggingFaceDatasetSource) -> Result<Self, String>;
    /// Resolve one public Hugging Face LeRobot v3 dataset without changing
    /// Spiral.
    async fn discover_lerobot(request: HuggingFaceDatasetSource) -> Result<Self, String>;
}

impl HuggingFaceDatasetDiscovery for HuggingFaceDatasetInspection {
    async fn discover(request: HuggingFaceDatasetSource) -> Result<Self, String> {
        HuggingFaceFileSelection::McapRecordings
            .inspect(request)
            .await
    }
    async fn discover_lerobot(request: HuggingFaceDatasetSource) -> Result<Self, String> {
        HuggingFaceFileSelection::LeRobot.inspect(request).await
    }
}

/// One pinned Hugging Face repository imported as a new dataset of one
/// format: downloaded and decoded 32 episodes at a time, each batch written
/// in its own transaction. A failed batch commits nothing; the batches
/// before it stay.
pub struct HuggingFaceImport {
    format: DatasetFormat,
    inspection: HuggingFaceDatasetInspection,
    dataset: DatasetSelection,
    downloads: HuggingFaceDownloads,
}

impl HuggingFaceImport {
    /// The repository `inspection` pinned, as a new dataset of `format`,
    /// read without a token.
    pub fn new(
        format: DatasetFormat,
        inspection: &HuggingFaceDatasetInspection,
    ) -> Result<Self, String> {
        Ok(Self {
            format,
            inspection: inspection.clone(),
            dataset: DatasetSelection::new(inspection.dataset_id())?,
            downloads: HuggingFaceDownloads::new(
                HuggingFaceRepository::try_from(inspection.repository())?,
                inspection.resolved_revision(),
            )?,
        })
    }

    /// The same import, every download sending `token`, the deployment's
    /// Hugging Face token, when there is one.
    pub fn with_token(mut self, token: Option<&str>) -> Self {
        self.downloads = self.downloads.with_token(token);
        self
    }

    /// Import, saying how far it has got: listing the repository, then the
    /// episodes committed after each batch.
    pub async fn run(
        self,
        storage: &StorageManager,
        progress: &(dyn Fn(DatasetImportPhase) + Send + Sync),
    ) -> Result<(), String> {
        progress(DatasetImportPhase::ListingFiles);
        let store = storage.project_store();
        if store
            .dataset_catalog()
            .datasets()
            .await?
            .holds(&self.dataset)
        {
            return Err(format!(
                "{} already exists; choose another dataset name or delete it first",
                self.dataset.label()
            ));
        }
        let mut import = DatasetImport::new(store, &self.dataset, self.format);
        let mut source = match self.format {
            DatasetFormat::Mcap => RepositoryEpisodes::Mcap(self.inspection.files()),
            DatasetFormat::LeRobot => RepositoryEpisodes::LeRobot(Box::new(
                LeRobotRepository::open(&self.downloads, self.inspection.files()).await?,
            )),
        };
        let total = source.len();
        let mut written = 0;
        progress(DatasetImportPhase::ImportingEpisodes {
            written: 0,
            total: total as u64,
        });
        // Each batch is read while the one before it commits: reading is
        // downloads and FFmpeg, committing is Spiral's, and neither waits on
        // the other's resources.
        let mut read = source
            .batch(&self.downloads, Self::batch_after(0, total))
            .await?;
        while !read.is_empty() {
            let committing = read.len();
            let next = Self::batch_after(written + committing, total);
            let (committed, following) = tokio::join!(
                import.commit(std::mem::take(&mut read)),
                source.batch(&self.downloads, next)
            );
            committed?;
            written += committing;
            progress(DatasetImportPhase::ImportingEpisodes {
                written: written as u64,
                total: total as u64,
            });
            read = following?;
        }
        Ok(())
    }

    /// The positions of the batch that starts at `start`, of `total`.
    fn batch_after(start: usize, total: usize) -> std::ops::Range<usize> {
        start.min(total)..total.min(start + DatasetWrite::MAX_EPISODES)
    }
}

/// A repository's episodes, read one at a time in order.
enum RepositoryEpisodes<'a> {
    /// Each `.mcap` file one episode.
    Mcap(&'a [HuggingFaceDatasetFile]),
    LeRobot(Box<LeRobotRepository<'a>>),
}

impl RepositoryEpisodes<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Mcap(files) => files.len(),
            Self::LeRobot(repository) => repository.len(),
        }
    }

    /// The episodes at `positions`, read from `downloads` one after another.
    async fn batch(
        &mut self,
        downloads: &HuggingFaceDownloads,
        positions: std::ops::Range<usize>,
    ) -> Result<Vec<DecodedEpisode>, String> {
        let mut episodes = Vec::with_capacity(positions.len());
        for position in positions {
            episodes.push(self.episode(downloads, position).await?);
        }
        Ok(episodes)
    }

    /// The episode at `position`, read from `downloads`.
    async fn episode(
        &mut self,
        downloads: &HuggingFaceDownloads,
        position: usize,
    ) -> Result<DecodedEpisode, String> {
        match self {
            Self::Mcap(files) => {
                let path = files[position].path();
                downloads
                    .mcap_episode(path, downloads.bytes(path).await?)
                    .await
            }
            Self::LeRobot(repository) => repository.episode(position).await,
        }
    }
}

trait HuggingFaceDatasetSourceValidation {
    fn validate_for_discovery(&self) -> Result<(), String>;
}

impl HuggingFaceDatasetSourceValidation for HuggingFaceDatasetSource {
    fn validate_for_discovery(&self) -> Result<(), String> {
        DatasetSelection::new(self.dataset_id())?;
        let revision = self.revision().trim();
        if revision.is_empty()
            || revision.len() > 200
            || revision.starts_with('/')
            || revision
                .split('/')
                .any(|segment| segment.is_empty() || segment == "..")
            || !revision.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
            })
        {
            return Err(
                "revision must contain 1–200 letters, numbers, '.', '_', '-', or '/' without empty or parent segments"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// Which files of a repository an import reads.
#[derive(Clone, Copy)]
enum HuggingFaceFileSelection {
    /// Every `.mcap` file, each one episode.
    McapRecordings,
    /// The whole repository of a LeRobot v3 dataset, which
    /// `meta/info.json` describes.
    LeRobot,
}

impl HuggingFaceFileSelection {
    /// The repository `request` names at the commit its revision resolves
    /// to, with the files this selection reads and what importing them
    /// creates.
    async fn inspect(
        self,
        request: HuggingFaceDatasetSource,
    ) -> Result<HuggingFaceDatasetInspection, String> {
        request.validate_for_discovery()?;
        let dataset = DatasetSelection::new(request.dataset_id())?;
        let repository = HuggingFaceRepository::try_from(request.repository())?;
        let client = HuggingFaceRepository::client()?;
        let info_url = repository.api_url("revision", request.revision())?;
        let response = client
            .get(info_url)
            .send()
            .await
            .map_err(|error| format!("contacting Hugging Face: {error}"))?;
        let response = response
            .require_hugging_face_success(repository.as_str())
            .await?;
        let info = response
            .json::<RepositoryInfo>()
            .await
            .map_err(|error| format!("decoding Hugging Face repository metadata: {error}"))?;
        if info.sha.is_empty()
            || !info
                .sha
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err("Hugging Face returned an invalid resolved commit".to_owned());
        }

        let entries = repository.tree(&client, &info.sha).await?;
        let selected = self.select(entries)?;
        let total_bytes = selected.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.bytes())
                .ok_or_else(|| "Hugging Face file sizes overflow u64".to_owned())
        })?;
        let mut warnings = Vec::new();
        if selected.iter().any(|file| file.bytes() == 0) {
            warnings.push(
            "Hugging Face did not report a byte size for every selected file; the displayed total is a lower bound."
                .to_owned(),
        );
        }
        let source = HuggingFaceInspectionSource::new(dataset.id(), repository.0)
            .with_resolved_revision(info.sha);
        let contents =
            HuggingFaceInspectionContents::new(selected, total_bytes, self.planned(&dataset));
        Ok(HuggingFaceDatasetInspection::new(source, contents).with_warnings(warnings))
    }

    /// The tables importing into `dataset` creates, beside one per stream.
    fn planned(self, dataset: &DatasetSelection) -> Vec<String> {
        let mut tables = vec![dataset.episodes(), dataset.topics()];
        if let Self::McapRecordings = self {
            tables.push(dataset.files());
        }
        tables.iter().map(|table| table.target()).collect()
    }

    fn select(self, entries: Vec<TreeEntry>) -> Result<Vec<HuggingFaceDatasetFile>, String> {
        let mut selected = entries
            .into_iter()
            .filter(|entry| {
                entry.kind == "file"
                    && match self {
                        Self::McapRecordings => entry.path.to_ascii_lowercase().ends_with(".mcap"),
                        Self::LeRobot => true,
                    }
            })
            .map(|entry| HuggingFaceDatasetFile::new(entry.path, entry.size))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.path().cmp(right.path()));
        selected.dedup_by(|left, right| left.path() == right.path());
        match self {
            Self::McapRecordings if selected.is_empty() => Err("No .mcap files were found at that revision. Dataset imports require MCAP recordings.".to_owned()),
            Self::LeRobot if !selected.iter().any(|file| file.path() == "meta/info.json") => Err("No meta/info.json was found at that revision; a LeRobot v3 dataset describes itself there.".to_owned()),
            Self::McapRecordings | Self::LeRobot => Ok(selected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(path: &str) -> TreeEntry {
        TreeEntry {
            kind: "file".into(),
            path: path.into(),
            size: 12,
        }
    }
    #[test]
    fn imports_only_mcap_files() {
        let mcap = HuggingFaceFileSelection::McapRecordings;
        assert!(mcap.select(vec![entry("data/frame.parquet")]).is_err());
        assert_eq!(
            mcap.select(vec![entry("recording.MCAP")]).unwrap()[0].bytes(),
            12
        );
    }
    #[test]
    fn a_lerobot_import_lists_the_whole_repository_it_describes() {
        let lerobot = HuggingFaceFileSelection::LeRobot;
        let directory = TreeEntry {
            kind: "directory".into(),
            ..entry("videos")
        };
        let files = lerobot
            .select(vec![
                entry("videos/observation.images.up/chunk-000/file-000.mp4"),
                directory,
                entry("meta/info.json"),
                entry("data/chunk-000/file-000.parquet"),
            ])
            .unwrap();
        assert_eq!(
            files
                .iter()
                .map(HuggingFaceDatasetFile::path)
                .collect::<Vec<_>>(),
            [
                "data/chunk-000/file-000.parquet",
                "meta/info.json",
                "videos/observation.images.up/chunk-000/file-000.mp4"
            ]
        );
        assert!(
            lerobot
                .select(vec![entry("data/chunk-000/file-000.parquet")])
                .unwrap_err()
                .contains("meta/info.json")
        );
    }
    #[test]
    fn files_are_pinned_to_the_commit_their_import_resolved() {
        let repository = HuggingFaceRepository::try_from("lerobot/svla_so101_pickplace").unwrap();
        let commit = "f641879e22172be7e8161d5e6c1503c2d2feb657";
        assert_eq!(
            repository.file_uri(commit, "").unwrap(),
            "hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/"
        );
        assert_eq!(
            repository
                .file_uri(commit, "episodes/take one.mcap")
                .unwrap(),
            "hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/episodes/take%20one.mcap"
        );
    }
    #[test]
    fn an_import_plans_the_tables_under_its_namespace() {
        let dataset = DatasetSelection::new("pick-place").unwrap();
        assert_eq!(
            HuggingFaceFileSelection::McapRecordings.planned(&dataset),
            [
                "datasets.pick-place.episodes",
                "datasets.pick-place.topics",
                "datasets.pick-place.files"
            ]
        );
        assert_eq!(
            HuggingFaceFileSelection::LeRobot.planned(&dataset),
            ["datasets.pick-place.episodes", "datasets.pick-place.topics"]
        );
        assert!(
            HuggingFaceDatasetSource::new("pick.place", "org/data")
                .validate_for_discovery()
                .is_err()
        );
    }
    #[test]
    fn old_format_requests_are_rejected() {
        assert!(serde_json::from_value::<HuggingFaceDatasetSource>(serde_json::json!({"dataset_id":"test", "repository":"org/data", "format":"lerobot"})).is_err());
    }
    #[test]
    fn repository_and_revision_are_validated() {
        assert!(HuggingFaceRepository::try_from("https://example.com/data").is_err());
        assert!(
            HuggingFaceDatasetSource::new("recordings", "org/data")
                .with_revision("../main")
                .validate_for_discovery()
                .is_err()
        );
    }
}
