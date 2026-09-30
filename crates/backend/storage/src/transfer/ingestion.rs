//! Hugging Face discovery and Spiral dataset materialization.
//!
//! Studio owns only repository inspection and catalog admission. Remote byte
//! reads and MCAP parsing remain inside Spiral, so imported data has the same
//! durable relation semantics as every other dataset.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use reqwest::StatusCode;
use reqwest::header::LINK;
use serde::Deserialize;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetFile, HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection,
    HuggingFaceDatasetSource, HuggingFaceInspectionContents, HuggingFaceInspectionSource,
};
use spiral_robotics_common::sql::SqlLiteral;
use url::Url;

use crate::datasets::{McapDatasetDefinition, McapSourceBlob};
use crate::db::ProjectConnection;
use crate::manager::StorageManager;

const HUGGING_FACE_ORIGIN: &str = "https://huggingface.co";
const MAX_TREE_PAGES: usize = 100;
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
        first_page
            .query_pairs_mut()
            .extend_pairs([("recursive", "true"), ("expand", "true")]);
        let mut next = Some(first_page);
        let mut files = Vec::new();
        for _ in 0..MAX_TREE_PAGES {
            let Some(url) = next.take() else {
                return Ok(files);
            };
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
        Err(format!(
            "Hugging Face file listing exceeded {MAX_TREE_PAGES} pages"
        ))
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
    /// Resolve one public Hugging Face dataset snapshot without changing Spiral.
    async fn discover(request: HuggingFaceDatasetSource) -> Result<Self, String>;
}

impl HuggingFaceDatasetDiscovery for HuggingFaceDatasetInspection {
    async fn discover(request: HuggingFaceDatasetSource) -> Result<Self, String> {
        request.validate_for_discovery()?;
        let dataset_id = request.dataset_id().to_owned();
        let requested_revision = request.revision().to_owned();
        let repository = HuggingFaceRepository::try_from(request.repository())?;
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "spiral-robotics-studio/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| format!("creating the Hugging Face client: {error}"))?;
        let info_url = repository.api_url("revision", &requested_revision)?;
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
        let selected = HuggingFaceFileSelection::select(entries)?;
        let total_bytes = selected.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.bytes())
                .ok_or_else(|| "Hugging Face file sizes overflow u64".to_owned())
        })?;
        let relations = McapDatasetDefinition::new(&dataset_id);
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
        Ok(Self::new(source, contents).with_warnings(warnings))
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
        let resources = self.project_resources().await?;
        if resources
            .iter()
            .any(|resource| relations.planned().contains(&resource.selection().label()))
        {
            return Err(
                "The dataset or its source already exists; choose another dataset name".to_owned(),
            );
        }
        let mut connection = self.project_store().connect_exclusive().await?;
        connection
            .transaction("Hugging Face import", async |connection| {
                inspection.materialize(connection, &relations).await
            })
            .await?;
        Ok(HuggingFaceDatasetImportSummary::new(
            inspection.files().len() as u64,
            relations.planned(),
        )
        .with_warnings(inspection.warnings().to_vec()))
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

struct HuggingFaceFileSelection;

impl HuggingFaceFileSelection {
    fn select(entries: Vec<TreeEntry>) -> Result<Vec<HuggingFaceDatasetFile>, String> {
        let mut selected = entries
            .into_iter()
            .filter(|entry| {
                entry.kind == "file" && entry.path.to_ascii_lowercase().ends_with(".mcap")
            })
            .map(|entry| HuggingFaceDatasetFile::new(entry.path, entry.size))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.path().cmp(right.path()));
        selected.dedup_by(|left, right| left.path() == right.path());
        if selected.is_empty() {
            return Err("No .mcap files were found at that revision. Dataset imports require MCAP recordings.".to_owned());
        }
        Ok(selected)
    }
}

trait HuggingFaceInspectionStorage {
    async fn materialize(
        &self,
        connection: &mut ProjectConnection,
        relations: &McapDatasetDefinition,
    ) -> Result<(), String>;
}
impl HuggingFaceInspectionStorage for HuggingFaceDatasetInspection {
    async fn materialize(
        &self,
        connection: &mut ProjectConnection,
        relations: &McapDatasetDefinition,
    ) -> Result<(), String> {
        // Orphaned source rows must not silently become a second import with
        // duplicate member keys after a Dataset or View was removed manually.
        if connection
            .table_exists(McapDatasetDefinition::SOURCE_TABLE)
            .await?
        {
            let rows = connection
                .query_rows(
                    &format!(
                        "SELECT COUNT(*) AS sources FROM {} WHERE dataset_name = {}",
                        McapDatasetDefinition::SOURCE_TABLE,
                        SqlLiteral::text(self.dataset_id())
                    ),
                    1,
                )
                .await?;
            if rows
                .rows()
                .first()
                .and_then(|row| row.get("sources"))
                .and_then(|value| value.as_u64())
                != Some(0)
            {
                return Err(
                    "Source rows already exist for this dataset; choose another dataset name"
                        .to_owned(),
                );
            }
        }
        connection
            .create_or_append_batches(
                McapDatasetDefinition::SOURCE_TABLE,
                vec![HuggingFaceSourceBatch::build(self)?],
            )
            .await?;
        relations.publish(connection).await?;
        Ok(())
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
        let schema = Arc::new(Schema::new(vec![
            Field::new("dataset_name", DataType::Utf8, false),
            Field::new("recording", DataType::Utf8, false),
            McapSourceBlob::field("source"),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from_iter_values(
                    files.iter().map(|_| inspection.dataset_id()),
                )) as ArrayRef,
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
    #[test]
    fn imports_only_mcap_files() {
        let entries = vec![TreeEntry {
            kind: "file".into(),
            path: "data/frame.parquet".into(),
            size: 12,
        }];
        assert!(HuggingFaceFileSelection::select(entries).is_err());
        let entries = vec![TreeEntry {
            kind: "file".into(),
            path: "recording.MCAP".into(),
            size: 12,
        }];
        assert_eq!(
            HuggingFaceFileSelection::select(entries).unwrap()[0].bytes(),
            12
        );
    }
    #[test]
    fn import_uses_native_producer_and_quoted_names() {
        let target = McapDatasetDefinition::new("pick-place");
        assert!(
            target
                .create_sql()
                .contains("CREATE DATASET \"dataset_pick-place\" USING spql.mcap.dataset")
        );
        assert_eq!(
            target.planned(),
            [
                "dataset_pick-place",
                "dataset_source_pick-place",
                "dataset_answers_pick-place"
            ]
        );
        assert_eq!(
            target.source_view_sql(),
            "CREATE VIEW \"dataset_source_pick-place\" AS SELECT recording, source FROM dataset_source WHERE dataset_name = 'pick-place'"
        );
    }
    #[test]
    fn shared_source_rows_retain_dataset_identity_and_pinned_blob_uri() {
        let source = HuggingFaceInspectionSource::new("pick-place", "org/recordings")
            .with_resolved_revision("abcdef");
        let contents = HuggingFaceInspectionContents::new(
            vec![HuggingFaceDatasetFile::new("episode.mcap", 12)],
            12,
            McapDatasetDefinition::new("pick-place").planned(),
        );
        let inspection = HuggingFaceDatasetInspection::new(source, contents);
        let batch = HuggingFaceSourceBatch::build(&inspection).unwrap();
        let names = batch.column_by_name("dataset_name").unwrap();
        assert_eq!(
            names
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0),
            "pick-place"
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
