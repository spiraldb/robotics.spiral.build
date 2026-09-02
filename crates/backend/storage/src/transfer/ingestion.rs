//! Hugging Face discovery and Spiral-native dataset materialization.
//!
//! Studio owns only repository inspection and catalog admission. Remote byte
//! reads, Parquet decoding, MP4 indexing, and MCAP parsing remain inside
//! Spiral so imported data has the same durable relation semantics as every
//! other Data/Workflow input.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Table;
use arrow_array::{
    ArrayRef, BinaryViewArray, RecordBatch, StringArray, StringViewArray, StructArray, UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema};
use reqwest::StatusCode;
use reqwest::header::LINK;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetFile, HuggingFaceDatasetFormat, HuggingFaceDatasetImportSummary,
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource, HuggingFaceImportOutcome,
    HuggingFaceInspectionContents, HuggingFaceInspectionSource, HuggingFaceResolvedSource,
};
use url::Url;

use crate::db::ProjectConnection;
use crate::db::SpiralSql;
use crate::episodes::EpisodeBundleSection;
use crate::manager::StorageManager;

const HUGGING_FACE_ORIGIN: &str = "https://huggingface.co";
const MAX_TREE_PAGES: usize = 100;
const MAX_REPOSITORY_FILES: usize = 50_000;
const BLOB_EXTENSION_NAME: &str = "spql.io.Blob";
const MCAP_OUTPUTS: [&str; 10] = [
    "files",
    "topics",
    "schemas",
    "channels",
    "chunks",
    "messages",
    "attachments",
    "metadata",
    "indexes",
    "quarantine",
];

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
        let mut next = Some(self.api_url("tree", revision)?);
        let mut files = Vec::new();
        for _ in 0..MAX_TREE_PAGES {
            let Some(url) = next.take() else {
                return Ok(files);
            };
            let response = client
                .get(url)
                .query(&[("recursive", "true"), ("expand", "true")])
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

#[derive(Clone, Debug)]
struct ImportRelations {
    base: String,
    source: Option<String>,
    primary: String,
    episodes: Option<String>,
    tasks: Option<String>,
    video_source: Option<String>,
    video_dataset: Option<String>,
}

#[derive(Clone, Copy)]
struct ImportTarget<'a> {
    namespace: &'a str,
    dataset_id: &'a str,
}

impl ImportTarget<'_> {
    fn relations(
        self,
        format: HuggingFaceDatasetFormat,
        files: &[HuggingFaceDatasetFile],
    ) -> ImportRelations {
        let namespace = self.namespace;
        let dataset_id = self.dataset_id;
        let slug = dataset_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .take(32)
            .collect::<String>();
        let digest = Sha256::digest(dataset_id.as_bytes());
        let hash = digest[..4]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let base = format!("{namespace}.hf_{slug}_{hash}");
        match format {
            HuggingFaceDatasetFormat::LeRobot => {
                let has_episodes = files
                    .iter()
                    .any(|file| LeRobotFile::is_episode_metadata(file.path()));
                let has_tasks = files.iter().any(|file| file.path() == "meta/tasks.parquet");
                let has_video = files.iter().any(|file| LeRobotFile::is_video(file.path()));
                ImportRelations {
                    primary: format!("{base}_frames"),
                    episodes: has_episodes.then(|| format!("{base}_episodes")),
                    tasks: has_tasks.then(|| format!("{base}_tasks")),
                    video_source: has_video.then(|| format!("{base}_video_sources")),
                    video_dataset: has_video.then(|| format!("{base}_video")),
                    base,
                    source: None,
                }
            }
            HuggingFaceDatasetFormat::Mcap => ImportRelations {
                source: Some(format!("{base}_sources")),
                primary: base.clone(),
                episodes: None,
                tasks: None,
                video_source: None,
                video_dataset: None,
                base,
            },
        }
    }
}

struct ImportCollision<'a> {
    namespace: &'a str,
    dataset_id: &'a str,
    relations: &'a ImportRelations,
}

impl ImportCollision<'_> {
    async fn refuse(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let datasets = Table::Datasets.qualified_name(self.namespace);
        if !connection
            .query_rows(&SpiralSql::dataset_exists(&datasets, self.dataset_id), 1)
            .await?
            .rows()
            .is_empty()
        {
            return Err(format!(
                "dataset {:?} already exists; choose another dataset ID",
                self.dataset_id
            ));
        }
        if let Some(existing) = connection
            .list_catalog_relations()
            .await?
            .into_iter()
            .map(|relation| relation.into_name())
            .find(|name| name.starts_with(&self.relations.base))
        {
            return Err(format!(
                "Spiral relation {existing} already occupies this import target; choose another dataset ID"
            ));
        }
        Ok(())
    }
}

impl ImportRelations {
    fn planned(&self, format: HuggingFaceDatasetFormat) -> Vec<String> {
        match format {
            HuggingFaceDatasetFormat::LeRobot => {
                let mut relations = vec![self.primary.clone()];
                relations.extend(self.episodes.iter().cloned());
                relations.extend(self.tasks.iter().cloned());
                relations.extend(self.video_source.iter().cloned());
                if let Some(dataset) = &self.video_dataset {
                    relations.push(format!("{dataset}.assets"));
                }
                relations
            }
            HuggingFaceDatasetFormat::Mcap => {
                let mut relations = self.source.iter().cloned().collect::<Vec<_>>();
                relations.extend(
                    MCAP_OUTPUTS
                        .iter()
                        .map(|output| format!("{}.{output}", self.primary)),
                );
                relations
            }
        }
    }
}

/// Storage-backed discovery behavior for the API inspection result type.
#[allow(async_fn_in_trait)]
pub trait HuggingFaceDatasetDiscovery: Sized {
    /// Resolve one public Hugging Face dataset snapshot without changing Spiral.
    async fn discover(request: HuggingFaceDatasetSource, namespace: &str) -> Result<Self, String>;
}

impl HuggingFaceDatasetDiscovery for HuggingFaceDatasetInspection {
    async fn discover(request: HuggingFaceDatasetSource, namespace: &str) -> Result<Self, String> {
        request.validate_for_discovery()?;
        let dataset_id = request.dataset_id().to_owned();
        let requested_revision = request.revision().to_owned();
        let format = request.format();
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
        let selected = HuggingFaceFileSelection::select(format, entries)?;
        let total_bytes = selected.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.bytes())
                .ok_or_else(|| "Hugging Face file sizes overflow u64".to_owned())
        })?;
        let relations = ImportTarget {
            namespace,
            dataset_id: &dataset_id,
        }
        .relations(format, &selected);
        let mut warnings = Vec::new();
        if selected.iter().any(|file| file.bytes() == 0) {
            warnings.push(
            "Hugging Face did not report a byte size for every selected file; the displayed total is a lower bound."
                .to_owned(),
        );
        }
        match format {
        HuggingFaceDatasetFormat::LeRobot => {
            let has_episode_metadata = selected
                .iter()
                .any(|file| LeRobotFile::is_episode_metadata(file.path()));
            let has_video = selected.iter().any(|file| LeRobotFile::is_video(file.path()));
            if !has_episode_metadata {
                warnings.push(
                    "No meta/episodes Parquet files were found; this looks like a LeRobot 2.x snapshot, so Studio will materialize frame data without a separate episode-metadata relation."
                        .to_owned(),
                );
            }
            if has_video {
                warnings.push(
                    "Video files will be indexed by Spiral as native MP4 media; frame data and video assets remain separate queryable relations."
                        .to_owned(),
                );
            }
        }
        HuggingFaceDatasetFormat::Mcap => warnings.push(
            "Spiral will quarantine an unreadable MCAP file and continue importing the remaining recordings. Topic-specific output relations are discovered while Spiral parses the files."
                .to_owned(),
        ),
    }

        let source = HuggingFaceInspectionSource::new(dataset_id, repository.0, format)
            .with_revisions(requested_revision, info.sha);
        let contents =
            HuggingFaceInspectionContents::new(selected, total_bytes, relations.planned(format));
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
        let namespace = self.project_store().namespace().to_owned();
        let relations = ImportTarget {
            namespace: &namespace,
            dataset_id: inspection.dataset_id(),
        }
        .relations(inspection.format(), inspection.files());
        let expected_prefix = relations.base.clone();
        let dataset_id = inspection.dataset_id().to_owned();
        let mut warnings = inspection.warnings().to_vec();
        let mut connection = self.project_store().connect_exclusive().await?;
        if let Err(error) = connection.begin_transaction().await {
            connection.discard();
            return Err(format!(
                "starting Hugging Face import transaction failed and its connection was discarded: {error}"
            ));
        }
        let mutation = async {
            ImportCollision {
                namespace: &namespace,
                dataset_id: &dataset_id,
                relations: &relations,
            }
            .refuse(&mut connection)
            .await?;
            inspection.materialize(&mut connection, &relations).await?;
            connection
                .ingest_batch(
                    &Table::Datasets.qualified_name(&namespace),
                    inspection.catalog_batch()?,
                )
                .await
        }
        .await;
        let connection_reopened = match mutation {
            Ok(()) => {
                if let Err(error) = connection.commit().await {
                    connection.discard();
                    return Err(format!(
                        "committing Hugging Face import failed with an uncertain outcome; connection discarded: {error}"
                    ));
                }
                match connection.reconnect().await {
                    Ok(()) => true,
                    Err(error) => {
                        connection.discard();
                        warnings.push(format!(
                            "Hugging Face rows committed, but the post-commit Spiral connection could not open; relation names are reported from the import plan: {error}"
                        ));
                        false
                    }
                }
            }
            Err(error) => {
                let rollback = connection.rollback().await;
                if rollback.is_err() {
                    connection.discard();
                }
                if let Err(rollback) = rollback {
                    return Err(format!(
                        "{error}; rolling back Hugging Face import also failed: {rollback}; connection discarded"
                    ));
                }
                return Err(error);
            }
        };

        let mut created = if connection_reopened {
            match connection.list_catalog_relations().await {
                Ok(relations) => relations
                    .into_iter()
                    .map(|relation| relation.into_name())
                    .filter(|name| name.starts_with(&expected_prefix))
                    .collect::<Vec<_>>(),
                Err(error) => {
                    warnings.push(format!(
                        "Hugging Face rows committed, but the imported relation catalog could not be inspected; relation names are reported from the import plan: {error}"
                    ));
                    relations.planned(inspection.format())
                }
            }
        } else {
            relations.planned(inspection.format())
        };
        created.sort();
        created.dedup();
        let suggested_sql = match inspection.format() {
            HuggingFaceDatasetFormat::LeRobot => format!(
                "SELECT * FROM {} ORDER BY episode_index, frame_index LIMIT 100",
                relations.primary
            ),
            HuggingFaceDatasetFormat::Mcap => format!(
                "SELECT * FROM {}.topics ORDER BY recording, topic LIMIT 100",
                relations.primary
            ),
        };
        let source = HuggingFaceResolvedSource::new(
            inspection.dataset_id(),
            inspection.repository(),
            inspection.format(),
        )
        .with_resolved_revision(inspection.resolved_revision());
        let outcome = HuggingFaceImportOutcome::new(
            inspection.files().len() as u64,
            inspection.total_bytes(),
            created,
        )
        .with_suggested_sql(suggested_sql);
        Ok(HuggingFaceDatasetImportSummary::new(source, outcome).with_warnings(warnings))
    }
}

trait HuggingFaceDatasetSourceValidation {
    fn validate_for_discovery(&self) -> Result<(), String>;
}

impl HuggingFaceDatasetSourceValidation for HuggingFaceDatasetSource {
    fn validate_for_discovery(&self) -> Result<(), String> {
        if !crate::db::StorageSchema::is_dataset_id(self.dataset_id()) {
            return Err(
                "dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'".to_owned(),
            );
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
    fn select(
        format: HuggingFaceDatasetFormat,
        entries: Vec<TreeEntry>,
    ) -> Result<Vec<HuggingFaceDatasetFile>, String> {
        let mut selected = entries
            .into_iter()
            .filter(|entry| entry.kind == "file")
            .filter(|entry| match format {
                HuggingFaceDatasetFormat::LeRobot => {
                    LeRobotFile::is_data(&entry.path)
                        || LeRobotFile::is_episode_metadata(&entry.path)
                        || entry.path == "meta/tasks.parquet"
                        || LeRobotFile::is_video(&entry.path)
                }
                HuggingFaceDatasetFormat::Mcap => {
                    entry.path.to_ascii_lowercase().ends_with(".mcap")
                }
            })
            .map(|entry| HuggingFaceDatasetFile::new(entry.path, entry.size))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.path().cmp(right.path()));
        selected.dedup_by(|left, right| left.path() == right.path());
        let valid = match format {
            HuggingFaceDatasetFormat::LeRobot => selected
                .iter()
                .any(|file| LeRobotFile::is_data(file.path())),
            HuggingFaceDatasetFormat::Mcap => !selected.is_empty(),
        };
        if !valid {
            return Err(match format {
                HuggingFaceDatasetFormat::LeRobot => {
                    "no LeRobot data/**/*.parquet files were found at that revision".to_owned()
                }
                HuggingFaceDatasetFormat::Mcap => {
                    "no .mcap files were found at that revision".to_owned()
                }
            });
        }
        Ok(selected)
    }
}

struct LeRobotFile;

impl LeRobotFile {
    fn is_data(path: &str) -> bool {
        path.starts_with("data/") && path.to_ascii_lowercase().ends_with(".parquet")
    }

    fn is_episode_metadata(path: &str) -> bool {
        path.starts_with("meta/episodes/") && path.to_ascii_lowercase().ends_with(".parquet")
    }

    fn is_video(path: &str) -> bool {
        path.starts_with("videos/") && path.to_ascii_lowercase().ends_with(".mp4")
    }
}

#[allow(async_fn_in_trait)]
trait HuggingFaceInspectionStorage {
    async fn materialize(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String>;
    fn catalog_batch(&self) -> Result<RecordBatch, String>;
    async fn materialize_lerobot(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String>;
    async fn materialize_mcap(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String>;
}

impl HuggingFaceInspectionStorage for HuggingFaceDatasetInspection {
    async fn materialize(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String> {
        match self.format() {
            HuggingFaceDatasetFormat::LeRobot => {
                self.materialize_lerobot(connection, relations).await
            }
            HuggingFaceDatasetFormat::Mcap => self.materialize_mcap(connection, relations).await,
        }
    }

    fn catalog_batch(&self) -> Result<RecordBatch, String> {
        let description = format!(
            "Hugging Face {} · {}@{}",
            self.format().label(),
            self.repository(),
            self.resolved_revision()
        );
        RecordBatch::try_new(
            EpisodeBundleSection::dataset_schema(),
            vec![
                Arc::new(StringArray::from(vec![self.dataset_id()])) as ArrayRef,
                Arc::new(StringArray::from(vec![self.dataset_id()])) as ArrayRef,
                Arc::new(arrow_array::Int64Array::from(vec![
                    IngestionClock::unix_millis(),
                ])) as ArrayRef,
                Arc::new(StringArray::from(vec![Some(description.as_str())])) as ArrayRef,
            ],
        )
        .map_err(|error| format!("building imported dataset catalog row: {error}"))
    }
    async fn materialize_lerobot(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String> {
        let base_uri = format!(
            "hf://datasets/{}@{}",
            self.repository(),
            self.resolved_revision()
        );
        connection
            .materialize_parquet_relation(
                &relations.primary,
                &format!("{base_uri}/data/**/*.parquet"),
            )
            .await?;
        if let Some(relation) = &relations.episodes {
            connection
                .materialize_parquet_relation(
                    relation,
                    &format!("{base_uri}/meta/episodes/**/*.parquet"),
                )
                .await?;
        }
        if let Some(relation) = &relations.tasks {
            connection
                .materialize_parquet_relation(relation, &format!("{base_uri}/meta/tasks.parquet"))
                .await?;
        }
        if let (Some(source), Some(dataset)) = (&relations.video_source, &relations.video_dataset) {
            let videos = self
                .files()
                .iter()
                .filter(|file| LeRobotFile::is_video(file.path()))
                .collect::<Vec<_>>();
            connection
                .ingest_batch(
                    source,
                    HuggingFaceSourceBatch::build(
                        self.repository(),
                        self.resolved_revision(),
                        &videos,
                    )?,
                )
                .await?;
            connection.execute_update_sql(&format!(
                "CREATE DATASET {dataset} USING spql.mp4.dataset FROM {source} WITH (blob = 'source', index_tier = 'dependency')"
            ),
        )
        .await
        .map_err(|error| format!("indexing LeRobot MP4 assets with Spiral: {error}"))?;
        }
        Ok(())
    }

    async fn materialize_mcap(
        &self,
        connection: &mut ProjectConnection,
        relations: &ImportRelations,
    ) -> Result<(), String> {
        let source = relations
            .source
            .as_deref()
            .ok_or_else(|| "MCAP import plan has no source relation".to_owned())?;
        let files = self.files().iter().collect::<Vec<_>>();
        connection
            .ingest_batch(
                source,
                HuggingFaceSourceBatch::build(self.repository(), self.resolved_revision(), &files)?,
            )
            .await?;
        connection.execute_update_sql(&format!(
            "CREATE DATASET {} USING spiral.mcap FROM {source} WITH (key = 'recording', blob = 'source')",
            relations.primary
        ),
    )
    .await
    .map_err(|error| format!("materializing MCAP files with Spiral: {error}"))
    }
}

impl ProjectConnection {
    async fn materialize_parquet_relation(
        &mut self,
        relation: &str,
        uri: &str,
    ) -> Result<(), String> {
        self.execute_update_sql(&format!(
            "CREATE TABLE {relation} ENGINE 'arbor' AS SELECT * FROM spql.parquet.read({})",
            SpiralSql::string_literal(uri)
        ))
        .await
        .map_err(|error| format!("materializing {relation} with Spiral Parquet: {error}"))
    }
}

struct HuggingFaceSourceBatch;

impl HuggingFaceSourceBatch {
    fn build(
        repository: &str,
        revision: &str,
        files: &[&HuggingFaceDatasetFile],
    ) -> Result<RecordBatch, String> {
        let uris = files
            .iter()
            .map(|file| {
                HuggingFaceRepository::try_from(repository)?.file_uri(revision, file.path())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let schema = Arc::new(Schema::new(vec![
            Field::new("recording", DataType::Utf8, false),
            ImportBlobStorage::field("source"),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from_iter_values(
                    files.iter().map(|file| file.path()),
                )) as ArrayRef,
                ImportBlobStorage::external_array(&uris, files)?,
            ],
        )
        .map_err(|error| format!("building Hugging Face source batch: {error}"))
    }
}

struct ImportBlobStorage;

impl ImportBlobStorage {
    fn fields() -> Fields {
        vec![
            Field::new("bytes", DataType::BinaryView, true),
            Field::new("uri", DataType::Utf8View, true),
            Field::new("offset", DataType::UInt64, true),
            Field::new("size", DataType::UInt64, true),
            Field::new("mime_type", DataType::Utf8View, true),
            Field::new("etag", DataType::Utf8View, true),
            Field::new("version_id", DataType::Utf8View, true),
            Field::new("generation", DataType::Utf8View, true),
            Field::new("md5", DataType::Utf8View, true),
            Field::new("sha1", DataType::Utf8View, true),
            Field::new("sha256", DataType::Utf8View, true),
            Field::new("manifest_digest", DataType::Utf8View, true),
        ]
        .into()
    }

    fn field(name: &str) -> Field {
        Field::new(name, DataType::Struct(Self::fields()), false).with_metadata(
            [(
                EXTENSION_TYPE_NAME_KEY.to_owned(),
                BLOB_EXTENSION_NAME.to_owned(),
            )]
            .into(),
        )
    }

    fn external_array(
        uris: &[String],
        files: &[&HuggingFaceDatasetFile],
    ) -> Result<ArrayRef, String> {
        let len = files.len();
        let null_text = || Arc::new(StringViewArray::from(vec![None::<&str>; len])) as ArrayRef;
        let null_u64 = || Arc::new(UInt64Array::from(vec![None::<u64>; len])) as ArrayRef;
        let values = vec![
            Arc::new(BinaryViewArray::from(vec![None::<&[u8]>; len])) as ArrayRef,
            Arc::new(StringViewArray::from_iter_values(
                uris.iter().map(String::as_str),
            )) as ArrayRef,
            null_u64(),
            Arc::new(UInt64Array::from_iter_values(
                files.iter().map(|file| file.bytes()),
            )) as ArrayRef,
            Arc::new(StringViewArray::from_iter_values(files.iter().map(
                |file| {
                    if file.path().to_ascii_lowercase().ends_with(".mcap") {
                        "application/mcap"
                    } else {
                        "video/mp4"
                    }
                },
            ))) as ArrayRef,
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
        ];
        StructArray::try_new(Self::fields(), values, None)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("building Spiral external Blob array: {error}"))
    }
}

struct IngestionClock;

impl IngestionClock {
    fn unix_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::datasets::{
        HuggingFaceDatasetFile, HuggingFaceDatasetFormat, HuggingFaceDatasetSource,
    };

    use super::{
        HuggingFaceDatasetSourceValidation as _, HuggingFaceFileSelection, HuggingFaceRepository,
        ImportTarget, TreeEntry,
    };

    #[test]
    fn repository_inputs_normalize_without_accepting_arbitrary_hosts() {
        assert_eq!(
            HuggingFaceRepository::try_from("lerobot/pusht")
                .unwrap()
                .as_str(),
            "lerobot/pusht",
        );
        assert_eq!(
            HuggingFaceRepository::try_from(
                "https://huggingface.co/datasets/lerobot/pusht/tree/main",
            )
            .unwrap()
            .as_str(),
            "lerobot/pusht",
        );
        assert!(HuggingFaceRepository::try_from("https://example.com/datasets/a/b").is_err());
        assert!(HuggingFaceRepository::try_from("lerobot/pusht/tree/main").is_err());
    }

    #[test]
    fn hub_api_paths_do_not_gain_an_empty_segment() {
        assert_eq!(
            HuggingFaceRepository::try_from("lerobot/pusht")
                .unwrap()
                .api_url("revision", "main")
                .unwrap()
                .as_str(),
            "https://huggingface.co/api/datasets/lerobot/pusht/revision/main"
        );
        assert_eq!(
            HuggingFaceRepository::try_from("lerobot/pusht")
                .unwrap()
                .file_uri("abc123", "videos/cam/file 0.mp4")
                .unwrap(),
            "hf://datasets/lerobot/pusht@abc123/videos/cam/file%200.mp4"
        );
    }

    #[test]
    fn revision_and_dataset_identity_are_bounded() {
        assert!(
            HuggingFaceDatasetSource::new(
                "bench-v1",
                "lerobot/pusht",
                HuggingFaceDatasetFormat::LeRobot,
            )
            .with_revision("refs/convert/parquet")
            .validate_for_discovery()
            .is_ok()
        );
        assert!(
            HuggingFaceDatasetSource::new(
                "bad id",
                "lerobot/pusht",
                HuggingFaceDatasetFormat::LeRobot,
            )
            .with_revision("../main")
            .validate_for_discovery()
            .is_err()
        );
    }

    #[test]
    fn format_selection_refuses_false_positive_repositories() {
        let entries = vec![TreeEntry {
            kind: "file".to_owned(),
            path: "data/chunk-000/file-000.parquet".to_owned(),
            size: 12,
        }];
        let files =
            HuggingFaceFileSelection::select(HuggingFaceDatasetFormat::LeRobot, entries).unwrap();
        assert_eq!(files[0].bytes(), 12);
        assert!(
            HuggingFaceFileSelection::select(
                HuggingFaceDatasetFormat::Mcap,
                vec![TreeEntry {
                    kind: "file".to_owned(),
                    path: "README.md".to_owned(),
                    size: 1,
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn relation_names_are_stable_and_scoped_to_the_studio_namespace() {
        let files = vec![HuggingFaceDatasetFile::new(
            "data/chunk-000/file-000.parquet",
            1,
        )];
        let target = ImportTarget {
            namespace: "spiral_robotics",
            dataset_id: "Pick.Place-v1",
        };
        let first = target.relations(HuggingFaceDatasetFormat::LeRobot, &files);
        let second = target.relations(HuggingFaceDatasetFormat::LeRobot, &files);
        assert_eq!(first.base, second.base);
        assert!(
            first
                .primary
                .starts_with("spiral_robotics.hf_pick_place_v1_")
        );
    }
}
