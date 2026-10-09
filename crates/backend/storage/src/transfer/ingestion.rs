//! Hugging Face discovery and Spiral dataset materialization.
//!
//! Studio owns only repository inspection and catalog admission. Remote byte
//! reads and MCAP parsing remain inside Spiral, so imported data has the same
//! durable relation semantics as every other dataset.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use reqwest::StatusCode;
use reqwest::header::LINK;
use serde::Deserialize;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetFile, HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection,
    HuggingFaceDatasetSource, HuggingFaceInspectionContents, HuggingFaceInspectionSource,
    LeRobotImportPhase,
};
use url::Url;

use crate::datasets::{
    LeRobotDatasetDefinition, McapDatasetDefinition, McapSourceBlob, StudioDataset,
};
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

    /// The repository's root at `revision`, as a Location names it.
    fn root_uri(&self, revision: &str) -> Result<String, String> {
        self.file_uri(revision, "")
    }

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

impl StorageManager {
    /// Atomically materialize one already pinned inspection into the active
    /// Spiral project.
    pub async fn import_huggingface_dataset(
        &self,
        inspection: HuggingFaceDatasetInspection,
    ) -> Result<HuggingFaceDatasetImportSummary, String> {
        let relations = McapDatasetDefinition::new(inspection.dataset_id());
        relations
            .relations()
            .refuse_existing(&self.project_resources().await?)?;
        let files = vec![HuggingFaceSourceBatch::build(&inspection)?];
        let mut connection = self.project_store().connect_exclusive().await?;
        connection
            .transaction("Hugging Face import", async |connection| {
                relations.publish(connection, files).await
            })
            .await?;
        Ok(HuggingFaceDatasetImportSummary::new(
            inspection.files().len() as u64,
            relations.relations().planned(),
        )
        .with_warnings(inspection.warnings().to_vec()))
    }

    /// Import the LeRobot dataset one pinned inspection found, in one
    /// transaction: a Location over the repository at its commit, the Dataset
    /// made from it, and its answers table, saying each phase as it starts.
    /// Spiral reads every episode and encodes its cameras again in this
    /// process, which takes as long as the dataset is big; the connection is
    /// shared, so the project stays readable meanwhile.
    pub async fn import_lerobot_dataset(
        &self,
        inspection: &HuggingFaceDatasetInspection,
        phase: &(dyn Fn(LeRobotImportPhase) + Send + Sync),
    ) -> Result<(), String> {
        let root = HuggingFaceRepository::try_from(inspection.repository())?
            .root_uri(inspection.resolved_revision())?;
        let definition = LeRobotDatasetDefinition::new(inspection.dataset_id(), root);
        definition
            .relations()
            .refuse_existing(&self.project_resources().await?)?;
        let mut connection = self.project_store().connect().await?;
        connection
            .transaction("LeRobot import", async |connection| {
                definition.import(connection, phase).await
            })
            .await
    }
}

trait HuggingFaceDatasetSourceValidation {
    fn validate_for_discovery(&self) -> Result<(), String>;
}

impl HuggingFaceDatasetSourceValidation for HuggingFaceDatasetSource {
    fn validate_for_discovery(&self) -> Result<(), String> {
        if !crate::db::StorageSchema::is_dataset_id(self.dataset_id())
            || self.dataset_id().contains('.')
        {
            return Err("dataset name must contain 1–96 letters, numbers, '_' or '-'".to_owned());
        }
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
        let dataset_id = request.dataset_id().to_owned();
        let repository = HuggingFaceRepository::try_from(request.repository())?;
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "spiral-robotics-studio/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| format!("creating the Hugging Face client: {error}"))?;
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
        let relations = StudioDataset::new(&dataset_id);
        let mut warnings = Vec::new();
        if selected.iter().any(|file| file.bytes() == 0) {
            warnings.push(
            "Hugging Face did not report a byte size for every selected file; the displayed total is a lower bound."
                .to_owned(),
        );
        }
        let source = HuggingFaceInspectionSource::new(dataset_id, repository.0)
            .with_resolved_revision(info.sha);
        let contents =
            HuggingFaceInspectionContents::new(selected, total_bytes, relations.planned());
        Ok(HuggingFaceDatasetInspection::new(source, contents).with_warnings(warnings))
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

struct HuggingFaceSourceBatch;

impl HuggingFaceSourceBatch {
    fn build(inspection: &HuggingFaceDatasetInspection) -> Result<RecordBatch, String> {
        let files = inspection.files().iter().collect::<Vec<_>>();
        let uris = files
            .iter()
            .map(|file| {
                HuggingFaceRepository::try_from(inspection.repository())?
                    .file_uri(inspection.resolved_revision(), file.path())
            })
            .collect::<Result<Vec<_>, _>>()?;
        RecordBatch::try_new(
            McapDatasetDefinition::files_schema(),
            vec![
                Arc::new(StringArray::from_iter_values(
                    files.iter().map(|file| file.path()),
                )) as ArrayRef,
                McapSourceBlob::external_array(
                    &uris,
                    &files.iter().map(|file| file.bytes()).collect::<Vec<_>>(),
                )?,
            ],
        )
        .map_err(|error| format!("building Hugging Face source batch: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{StringViewArray, StructArray};
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
    fn a_location_names_the_repository_root_at_its_commit() {
        let repository = HuggingFaceRepository::try_from("lerobot/svla_so101_pickplace").unwrap();
        assert_eq!(
            repository
                .root_uri("f641879e22172be7e8161d5e6c1503c2d2feb657")
                .unwrap(),
            "hf://datasets/lerobot/svla_so101_pickplace@f641879e22172be7e8161d5e6c1503c2d2feb657/"
        );
    }
    #[test]
    fn import_uses_native_producer_and_quoted_names() {
        let target = McapDatasetDefinition::new("pick-place");
        assert_eq!(
            target.create_sql(),
            "CREATE DATASET \"dataset_pick-place\" USING spql.mcap.dataset FROM \"dataset_files_pick-place\" WITH (key = 'recording', blob = 'source', on_error = 'fail', materialization = 'all')"
        );
        assert_eq!(
            target.relations().planned(),
            [
                "dataset_pick-place",
                "dataset_files_pick-place",
                "dataset_answers_pick-place"
            ]
        );
    }
    #[test]
    fn source_rows_keep_each_file_and_its_pinned_blob_uri() {
        let source = HuggingFaceInspectionSource::new("pick-place", "org/recordings")
            .with_resolved_revision("abcdef");
        let contents = HuggingFaceInspectionContents::new(
            vec![HuggingFaceDatasetFile::new("episode.mcap", 12)],
            12,
            StudioDataset::new("pick-place").planned(),
        );
        let inspection = HuggingFaceDatasetInspection::new(source, contents);
        let batch = HuggingFaceSourceBatch::build(&inspection).unwrap();
        assert_eq!(batch.schema(), McapDatasetDefinition::files_schema());
        let recordings = batch.column_by_name("recording").unwrap();
        assert_eq!(
            recordings
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0),
            "episode.mcap"
        );
        let blobs = batch.column_by_name("source").unwrap();
        let blobs = blobs.as_any().downcast_ref::<StructArray>().unwrap();
        let uris = blobs.column_by_name("uri").unwrap();
        assert_eq!(
            uris.as_any()
                .downcast_ref::<StringViewArray>()
                .unwrap()
                .value(0),
            "hf://datasets/org/recordings@abcdef/episode.mcap"
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
