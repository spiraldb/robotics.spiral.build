//! Hugging Face discovery and Spiral-native dataset materialization.
//!
//! Studio owns only repository inspection and catalog admission. Remote byte
//! reads, Parquet decoding, MP4 indexing, and MCAP parsing remain inside
//! Spiral so imported data has the same durable relation semantics as every
//! other Data/Workflow input.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{
    ArrayRef, BinaryViewArray, RecordBatch, StringArray, StringViewArray, StructArray, UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema};
use reqwest::StatusCode;
use reqwest::header::LINK;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use spiral_robotics_contracts::api::{
    HuggingFaceDatasetFile, HuggingFaceDatasetFormat, HuggingFaceDatasetImportSummary,
    HuggingFaceDatasetInspection, HuggingFaceDatasetRequest,
};
use spiral_robotics_contracts::spiral::Table;
use url::Url;

use super::manager::{
    StorageManager, commit_catalog_transaction, ensure_dataset_namespace, execute_update_sql,
    ingest_batch, run_query, table_exists,
};
use super::repository::RepositoryConnection;
use super::sql;
use crate::backend::recording::spiral::datasets_schema;

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

/// Resolve one public Hugging Face dataset snapshot without changing Spiral.
pub(crate) async fn inspect_huggingface_dataset(
    request: HuggingFaceDatasetRequest,
    namespace: &str,
) -> Result<HuggingFaceDatasetInspection, String> {
    validate_request(&request)?;
    let repository = normalize_repository(&request.repository)?;
    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "spiral-robotics-studio/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|error| format!("creating the Hugging Face client: {error}"))?;
    let info_url = hugging_face_api_url(&repository, "revision", &request.revision)?;
    let response = client
        .get(info_url)
        .send()
        .await
        .map_err(|error| format!("contacting Hugging Face: {error}"))?;
    let response = require_hugging_face_success(response, &repository).await?;
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

    let entries = repository_tree(&client, &repository, &info.sha).await?;
    let selected = select_files(request.format, entries)?;
    let total_bytes = selected.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.bytes)
            .ok_or_else(|| "Hugging Face file sizes overflow u64".to_owned())
    })?;
    let relations = import_relations(namespace, &request.dataset_id, request.format, &selected);
    let mut warnings = Vec::new();
    if selected.iter().any(|file| file.bytes == 0) {
        warnings.push(
            "Hugging Face did not report a byte size for every selected file; the displayed total is a lower bound."
                .to_owned(),
        );
    }
    match request.format {
        HuggingFaceDatasetFormat::LeRobot => {
            let has_episode_metadata = selected
                .iter()
                .any(|file| is_lerobot_episode_metadata(&file.path));
            let has_video = selected.iter().any(|file| is_lerobot_video(&file.path));
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

    Ok(HuggingFaceDatasetInspection {
        dataset_id: request.dataset_id,
        repository,
        requested_revision: request.revision,
        resolved_revision: info.sha,
        format: request.format,
        files: selected,
        total_bytes,
        planned_relations: relations.planned(request.format),
        warnings,
    })
}

impl StorageManager {
    /// Atomically materialize one already pinned inspection into the active
    /// Spiral project. Callers run this on a blocking worker because ADBC is
    /// synchronous and the import may perform substantial remote I/O.
    pub(crate) fn import_huggingface_dataset(
        &self,
        inspection: HuggingFaceDatasetInspection,
    ) -> Result<HuggingFaceDatasetImportSummary, String> {
        let namespace = self.repository.namespace().to_owned();
        let project = self.repository.project_id();
        let relations = import_relations(
            &namespace,
            &inspection.dataset_id,
            inspection.format,
            &inspection.files,
        );
        let expected_prefix = relations.base.clone();
        let dataset_id = inspection.dataset_id.clone();
        let result = self.repository.run_invalidating_blocking(move |connection| {
            ensure_dataset_namespace(connection, &namespace)?;
            connection.set_autocommit(false)?;
            let mutation = (|| {
                refuse_collision(connection, &project, &namespace, &dataset_id, &relations)?;
                match inspection.format {
                    HuggingFaceDatasetFormat::LeRobot => {
                        materialize_lerobot(connection, &inspection, &relations)?
                    }
                    HuggingFaceDatasetFormat::Mcap => {
                        materialize_mcap(connection, &inspection, &relations)?
                    }
                }
                ingest_batch(
                    connection,
                    &sql::relation(&namespace, Table::Datasets),
                    imported_dataset_batch(&inspection)?,
                )?;
                commit_catalog_transaction(connection)
            })();
            let rollback = mutation.is_err().then(|| connection.rollback());
            if rollback.as_ref().is_some_and(Result::is_err) {
                connection.discard();
            }
            let restore = connection.set_autocommit(true);
            if restore.is_err() {
                connection.discard();
            }
            match (mutation, rollback, restore) {
                (Ok(()), None, Ok(())) | (Ok(()), None, Err(_)) => {}
                (Err(error), Some(Ok(())), Ok(())) => return Err(error),
                (Err(error), Some(rollback), restore) => {
                    return Err(format!(
                        "{error}; import cleanup was uncertain (rollback: {rollback:?}, autocommit: {restore:?})"
                    ));
                }
                _ => unreachable!("recovery rollback follows the mutation outcome"),
            }

            let mut created = connection
                .list_catalog_relations(&project)?
                .into_iter()
                .map(|relation| relation.name)
                .filter(|name| name.starts_with(&expected_prefix))
                .collect::<Vec<_>>();
            created.sort();
            created.dedup();
            let suggested_sql = match inspection.format {
                HuggingFaceDatasetFormat::LeRobot => format!(
                    "SELECT * FROM {} ORDER BY episode_index, frame_index LIMIT 100",
                    relations.primary
                ),
                HuggingFaceDatasetFormat::Mcap => format!(
                    "SELECT * FROM {}.topics ORDER BY recording, topic LIMIT 100",
                    relations.primary
                ),
            };
            Ok(HuggingFaceDatasetImportSummary {
                dataset_id: inspection.dataset_id,
                repository: inspection.repository,
                resolved_revision: inspection.resolved_revision,
                format: inspection.format,
                files: inspection.files.len() as u64,
                total_bytes: inspection.total_bytes,
                relations: created,
                suggested_sql,
                warnings: inspection.warnings,
            })
        });
        result
    }
}

fn validate_request(request: &HuggingFaceDatasetRequest) -> Result<(), String> {
    if !spiral_robotics_contracts::spiral::is_dataset_id(&request.dataset_id) {
        return Err("dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'".to_owned());
    }
    let revision = request.revision.trim();
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

fn normalize_repository(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    let (candidate, allow_suffix) = if let Some(value) = trimmed.strip_prefix("hf://datasets/") {
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
    Ok(format!("{}/{}", segments[0], segments[1]))
}

fn hugging_face_api_url(repository: &str, operation: &str, revision: &str) -> Result<Url, String> {
    let mut url = Url::parse(&format!("{HUGGING_FACE_ORIGIN}/api/datasets/"))
        .map_err(|error| format!("building the Hugging Face API URL: {error}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "Hugging Face API URL cannot accept path segments".to_owned())?;
        segments.pop_if_empty();
        for segment in repository.split('/') {
            segments.push(segment);
        }
        segments.push(operation).push(revision);
    }
    Ok(url)
}

async fn repository_tree(
    client: &reqwest::Client,
    repository: &str,
    revision: &str,
) -> Result<Vec<TreeEntry>, String> {
    let mut next = Some(hugging_face_api_url(repository, "tree", revision)?);
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
            .map_err(|error| format!("listing Hugging Face repository files: {error}"))?;
        let response = require_hugging_face_success(response, repository).await?;
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
        next = link.as_deref().map(parse_next_link).transpose()?.flatten();
    }
    Err(format!(
        "Hugging Face file listing exceeded {MAX_TREE_PAGES} pages"
    ))
}

async fn require_hugging_face_success(
    response: reqwest::Response,
    repository: &str,
) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let detail = response.text().await.unwrap_or_default();
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

fn select_files(
    format: HuggingFaceDatasetFormat,
    entries: Vec<TreeEntry>,
) -> Result<Vec<HuggingFaceDatasetFile>, String> {
    let mut selected = entries
        .into_iter()
        .filter(|entry| entry.kind == "file")
        .filter(|entry| match format {
            HuggingFaceDatasetFormat::LeRobot => {
                is_lerobot_data(&entry.path)
                    || is_lerobot_episode_metadata(&entry.path)
                    || entry.path == "meta/tasks.parquet"
                    || is_lerobot_video(&entry.path)
            }
            HuggingFaceDatasetFormat::Mcap => entry.path.to_ascii_lowercase().ends_with(".mcap"),
        })
        .map(|entry| HuggingFaceDatasetFile {
            path: entry.path,
            bytes: entry.size,
        })
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| left.path.cmp(&right.path));
    selected.dedup_by(|left, right| left.path == right.path);
    let valid = match format {
        HuggingFaceDatasetFormat::LeRobot => {
            selected.iter().any(|file| is_lerobot_data(&file.path))
        }
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

fn is_lerobot_data(path: &str) -> bool {
    path.starts_with("data/") && path.to_ascii_lowercase().ends_with(".parquet")
}

fn is_lerobot_episode_metadata(path: &str) -> bool {
    path.starts_with("meta/episodes/") && path.to_ascii_lowercase().ends_with(".parquet")
}

fn is_lerobot_video(path: &str) -> bool {
    path.starts_with("videos/") && path.to_ascii_lowercase().ends_with(".mp4")
}

fn import_relations(
    namespace: &str,
    dataset_id: &str,
    format: HuggingFaceDatasetFormat,
    files: &[HuggingFaceDatasetFile],
) -> ImportRelations {
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
                .any(|file| is_lerobot_episode_metadata(&file.path));
            let has_tasks = files.iter().any(|file| file.path == "meta/tasks.parquet");
            let has_video = files.iter().any(|file| is_lerobot_video(&file.path));
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

fn refuse_collision(
    connection: &mut RepositoryConnection,
    project: &str,
    namespace: &str,
    dataset_id: &str,
    relations: &ImportRelations,
) -> Result<(), String> {
    let datasets = sql::relation(namespace, Table::Datasets);
    if table_exists(connection, &datasets)?
        && !run_query(connection, &sql::dataset_exists(&datasets, dataset_id), 1)?
            .rows
            .is_empty()
    {
        return Err(format!(
            "dataset {dataset_id:?} already exists; choose another dataset ID"
        ));
    }
    if let Some(existing) = connection
        .list_catalog_relations(project)?
        .into_iter()
        .map(|relation| relation.name)
        .find(|name| name.starts_with(&relations.base))
    {
        return Err(format!(
            "Spiral relation {existing} already occupies this import target; choose another dataset ID"
        ));
    }
    Ok(())
}

fn materialize_lerobot(
    connection: &mut RepositoryConnection,
    inspection: &HuggingFaceDatasetInspection,
    relations: &ImportRelations,
) -> Result<(), String> {
    let base_uri = format!(
        "hf://datasets/{}@{}",
        inspection.repository, inspection.resolved_revision
    );
    create_parquet_relation(
        connection,
        &relations.primary,
        &format!("{base_uri}/data/**/*.parquet"),
    )?;
    if let Some(relation) = &relations.episodes {
        create_parquet_relation(
            connection,
            relation,
            &format!("{base_uri}/meta/episodes/**/*.parquet"),
        )?;
    }
    if let Some(relation) = &relations.tasks {
        create_parquet_relation(
            connection,
            relation,
            &format!("{base_uri}/meta/tasks.parquet"),
        )?;
    }
    if let (Some(source), Some(dataset)) = (&relations.video_source, &relations.video_dataset) {
        let videos = inspection
            .files
            .iter()
            .filter(|file| is_lerobot_video(&file.path))
            .collect::<Vec<_>>();
        ingest_batch(
            connection,
            source,
            source_blob_batch(
                &inspection.repository,
                &inspection.resolved_revision,
                &videos,
            )?,
        )?;
        execute_update_sql(
            connection,
            &format!(
                "CREATE DATASET {dataset} USING spql.mp4.dataset FROM {source} WITH (blob = 'source', index_tier = 'dependency')"
            ),
        )
        .map_err(|error| format!("indexing LeRobot MP4 assets with Spiral: {error}"))?;
    }
    Ok(())
}

fn materialize_mcap(
    connection: &mut RepositoryConnection,
    inspection: &HuggingFaceDatasetInspection,
    relations: &ImportRelations,
) -> Result<(), String> {
    let source = relations
        .source
        .as_deref()
        .ok_or_else(|| "MCAP import plan has no source relation".to_owned())?;
    let files = inspection.files.iter().collect::<Vec<_>>();
    ingest_batch(
        connection,
        source,
        source_blob_batch(
            &inspection.repository,
            &inspection.resolved_revision,
            &files,
        )?,
    )?;
    execute_update_sql(
        connection,
        &format!(
            "CREATE DATASET {} USING spiral.mcap FROM {source} WITH (key = 'recording', blob = 'source')",
            relations.primary
        ),
    )
    .map_err(|error| format!("materializing MCAP files with Spiral: {error}"))
}

fn create_parquet_relation(
    connection: &mut RepositoryConnection,
    relation: &str,
    uri: &str,
) -> Result<(), String> {
    execute_update_sql(
        connection,
        &format!(
            "CREATE TABLE {relation} ENGINE 'arbor' AS SELECT * FROM spql.parquet.read({})",
            sql::string_literal(uri)
        ),
    )
    .map_err(|error| format!("materializing {relation} with Spiral Parquet: {error}"))
}

fn source_blob_batch(
    repository: &str,
    revision: &str,
    files: &[&HuggingFaceDatasetFile],
) -> Result<RecordBatch, String> {
    let uris = files
        .iter()
        .map(|file| hugging_face_uri(repository, revision, &file.path))
        .collect::<Result<Vec<_>, _>>()?;
    let schema = Arc::new(Schema::new(vec![
        Field::new("recording", DataType::Utf8, false),
        blob_field("source"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                files.iter().map(|file| file.path.as_str()),
            )) as ArrayRef,
            external_blob_array(&uris, files)?,
        ],
    )
    .map_err(|error| format!("building Hugging Face source batch: {error}"))
}

fn hugging_face_uri(repository: &str, revision: &str, path: &str) -> Result<String, String> {
    let mut url = Url::parse("hf://datasets/")
        .map_err(|error| format!("building a Spiral Hugging Face URI: {error}"))?;
    let mut repository_parts = repository.split('/');
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

fn blob_storage_fields() -> Fields {
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

fn blob_field(name: &str) -> Field {
    Field::new(name, DataType::Struct(blob_storage_fields()), false).with_metadata(
        [(
            EXTENSION_TYPE_NAME_KEY.to_owned(),
            BLOB_EXTENSION_NAME.to_owned(),
        )]
        .into(),
    )
}

fn external_blob_array(
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
            files.iter().map(|file| file.bytes),
        )) as ArrayRef,
        Arc::new(StringViewArray::from_iter_values(files.iter().map(
            |file| {
                if file.path.to_ascii_lowercase().ends_with(".mcap") {
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
    StructArray::try_new(blob_storage_fields(), values, None)
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| format!("building Spiral external Blob array: {error}"))
}

fn imported_dataset_batch(
    inspection: &HuggingFaceDatasetInspection,
) -> Result<RecordBatch, String> {
    let description = format!(
        "Hugging Face {} · {}@{}",
        inspection.format.label(),
        inspection.repository,
        inspection.resolved_revision
    );
    RecordBatch::try_new(
        datasets_schema(),
        vec![
            Arc::new(StringArray::from(vec![inspection.dataset_id.as_str()])) as ArrayRef,
            Arc::new(StringArray::from(vec![inspection.dataset_id.as_str()])) as ArrayRef,
            Arc::new(arrow_array::Int64Array::from(vec![unix_millis()])) as ArrayRef,
            Arc::new(StringArray::from(vec![Some(description.as_str())])) as ArrayRef,
        ],
    )
    .map_err(|error| format!("building imported dataset catalog row: {error}"))
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use spiral_robotics_contracts::api::{
        HuggingFaceDatasetFile, HuggingFaceDatasetFormat, HuggingFaceDatasetRequest,
    };

    use super::{
        TreeEntry, import_relations, normalize_repository, select_files, validate_request,
    };

    #[test]
    fn repository_inputs_normalize_without_accepting_arbitrary_hosts() {
        assert_eq!(
            normalize_repository("lerobot/pusht").unwrap(),
            "lerobot/pusht"
        );
        assert_eq!(
            normalize_repository("https://huggingface.co/datasets/lerobot/pusht/tree/main")
                .unwrap(),
            "lerobot/pusht"
        );
        assert!(normalize_repository("https://example.com/datasets/a/b").is_err());
        assert!(normalize_repository("lerobot/pusht/tree/main").is_err());
    }

    #[test]
    fn hub_api_paths_do_not_gain_an_empty_segment() {
        assert_eq!(
            super::hugging_face_api_url("lerobot/pusht", "revision", "main")
                .unwrap()
                .as_str(),
            "https://huggingface.co/api/datasets/lerobot/pusht/revision/main"
        );
        assert_eq!(
            super::hugging_face_uri("lerobot/pusht", "abc123", "videos/cam/file 0.mp4").unwrap(),
            "hf://datasets/lerobot/pusht@abc123/videos/cam/file%200.mp4"
        );
    }

    #[test]
    fn revision_and_dataset_identity_are_bounded() {
        assert!(
            validate_request(&HuggingFaceDatasetRequest {
                dataset_id: "bench-v1".to_owned(),
                repository: "lerobot/pusht".to_owned(),
                revision: "refs/convert/parquet".to_owned(),
                format: HuggingFaceDatasetFormat::LeRobot,
            })
            .is_ok()
        );
        assert!(
            validate_request(&HuggingFaceDatasetRequest {
                dataset_id: "bad id".to_owned(),
                repository: "lerobot/pusht".to_owned(),
                revision: "../main".to_owned(),
                format: HuggingFaceDatasetFormat::LeRobot,
            })
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
        let files = select_files(HuggingFaceDatasetFormat::LeRobot, entries).unwrap();
        assert_eq!(files[0].bytes, 12);
        assert!(
            select_files(
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
        let files = vec![HuggingFaceDatasetFile {
            path: "data/chunk-000/file-000.parquet".to_owned(),
            bytes: 1,
        }];
        let first = import_relations(
            "spiral_robotics",
            "Pick.Place-v1",
            HuggingFaceDatasetFormat::LeRobot,
            &files,
        );
        let second = import_relations(
            "spiral_robotics",
            "Pick.Place-v1",
            HuggingFaceDatasetFormat::LeRobot,
            &files,
        );
        assert_eq!(first.base, second.base);
        assert!(
            first
                .primary
                .starts_with("spiral_robotics.hf_pick_place_v1_")
        );
    }
}
