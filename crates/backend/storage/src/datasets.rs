//! Native Dataset discovery through the same project journal used by the ADBC driver.
#[cfg(test)]
mod catalog_tests;
mod sources;
mod topics;
pub(crate) use sources::{McapDatasetDefinition, McapFile, McapSourceBlob, McapSources};

use crate::{SpiralAccess, SpiralConnection, StorageManager};
use serde::Deserialize;
use serde_json::json;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetSummary, DatasetTopic, EpisodeId, ProjectResource,
};
use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

/// Reads a project's journal as the signed-in user.
pub(crate) struct DatasetCatalogClient {
    client: reqwest::Client,
    endpoint: String,
    project: String,
    access: SpiralAccess,
}

impl DatasetCatalogClient {
    pub(crate) fn new(connection: &SpiralConnection) -> Result<Self, String> {
        let settings = connection.settings();
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| error.to_string())?,
            endpoint: settings.endpoint().trim_end_matches('/').to_owned(),
            project: settings.project_id().unwrap_or_default().to_owned(),
            access: connection.access().clone(),
        })
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, String> {
        let mut response = request
            .bearer_auth(self.access.token()?)
            .send()
            .await
            .map_err(|error| format!("Reading Spiral dataset metadata: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Spiral dataset metadata request failed ({})",
                response.status()
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err("Spiral dataset metadata exceeds the 16 MiB response limit".to_owned());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("Decoding Spiral dataset metadata: {error}"))
    }

    pub(crate) async fn resources(&self) -> Result<Vec<ProjectResource>, String> {
        if self.project.is_empty() {
            return Err("Select a Spiral project".into());
        }
        let prefix = format!("{}/projects/{}/journal", self.endpoint, self.project);
        let branch: JournalBranch = self
            .request(self.client.get(format!("{prefix}/branches/main")))
            .await?;
        let mut namespace = Vec::new();
        let mut token = None::<String>;
        for _ in 0..100 {
            let page: JournalPage = self.request(self.client.post(format!("{prefix}/state/read")).json(&json!({ "commitId": branch.head_commit_id, "pageSize": 1000, "pageToken": token }))).await?;
            namespace.extend(page.namespace);
            token = page.next_page_token;
            if token.is_none() {
                return DatasetNamespace::new(namespace).resources();
            }
        }
        Err("Spiral project catalog exceeds the inspection limit".to_owned())
    }

    async fn datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        Ok(self
            .resources()
            .await?
            .iter()
            .filter_map(ProjectResource::dataset)
            .collect())
    }

    async fn resolve(&self, selection: &DatasetSelection) -> Result<DatasetSummary, String> {
        self.datasets()
            .await?
            .into_iter()
            .find(|dataset| dataset.selection() == selection)
            .ok_or_else(|| "Dataset no longer exists in this project; refresh the catalog".into())
    }

    async fn inspect(&self, selection: DatasetSelection) -> Result<DatasetInspection, String> {
        let dataset = self.resolve(&selection).await?;
        let response: DatasetVersionResponse = self
            .request(self.client.get(format!(
                "{}/datasets/{}/versions/{}",
                self.endpoint,
                dataset.resource_id(),
                dataset.version_id()
            )))
            .await?;
        let Some(revision) = response.version.data_revision else {
            return Ok(DatasetInspection::new(dataset, None, vec![]));
        };
        let mut topics = revision
            .data
            .schema
            .observations
            .into_iter()
            .map(DatasetObservation::topic)
            .collect::<Result<Vec<_>, _>>()?;
        topics.sort_by(|left, right| left.name().cmp(right.name()));
        Ok(DatasetInspection::new(
            dataset,
            Some(revision.data.member_count),
            topics,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalBranch {
    head_commit_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalPage {
    namespace: Vec<NamespaceEntry>,
    next_page_token: Option<String>,
}
#[derive(Deserialize)]
struct NamespaceEntry {
    parent: NamespaceParent,
    name: String,
    resource: DatasetResource,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum NamespaceParent {
    ProjectRoot,
    Resource {
        resource_id: String,
    },
    /// Implicit name segments relative to the project root or a resource.
    Prefix {
        resource_id: Option<String>,
        path: Vec<String>,
    },
}
impl NamespaceParent {
    fn resource_id(&self) -> Option<&str> {
        match self {
            Self::ProjectRoot => None,
            Self::Resource { resource_id } => Some(resource_id),
            Self::Prefix { resource_id, .. } => resource_id.as_deref(),
        }
    }

    fn path(&self) -> &[String] {
        match self {
            Self::ProjectRoot | Self::Resource { .. } => &[],
            Self::Prefix { path, .. } => path,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetResource {
    descriptor: ResourceDescriptor,
    version_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDescriptor {
    resource_id: String,
    kind: String,
}

struct DatasetNamespace {
    entries: Vec<NamespaceEntry>,
}
impl DatasetNamespace {
    fn new(entries: Vec<NamespaceEntry>) -> Self {
        Self { entries }
    }
    fn resources(self) -> Result<Vec<ProjectResource>, String> {
        let mut queue = VecDeque::from([(None::<String>, Vec::<String>::new())]);
        let mut visited = BTreeSet::new();
        let mut datasets = Vec::new();
        while let Some((parent, prefix)) = queue.pop_front() {
            for entry in &self.entries {
                if entry.parent.resource_id() != parent.as_deref() {
                    continue;
                }
                let mut path = prefix.clone();
                path.extend_from_slice(entry.parent.path());
                path.push(entry.name.clone());
                if path.len() > 32 {
                    return Err("Dataset namespace exceeds the nesting limit".to_owned());
                }
                let resource = &entry.resource;
                datasets.push(ProjectResource::new(
                    DatasetSelection::new(path.clone())?,
                    resource.descriptor.kind.clone(),
                    (
                        resource.descriptor.resource_id.clone(),
                        resource.version_id.clone(),
                    ),
                ));
                if visited.insert(resource.descriptor.resource_id.clone()) {
                    queue.push_back((Some(resource.descriptor.resource_id.clone()), path));
                }
            }
        }
        datasets.sort_by_key(|dataset| dataset.selection().label());
        Ok(datasets)
    }
}

#[derive(Deserialize)]
struct DatasetVersionResponse {
    version: DatasetVersion,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetVersion {
    data_revision: Option<DatasetRevision>,
}
#[derive(Deserialize)]
struct DatasetRevision {
    data: DatasetData,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetData {
    member_count: u64,
    schema: DatasetSchema,
}
#[derive(Deserialize)]
struct DatasetSchema {
    observations: Vec<DatasetObservation>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetObservation {
    name: String,
    variants: Vec<NativeVariant>,
    coverage: ObservationCoverage,
    scan_locator: ScanLocator,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeVariant {
    native_schema_name: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservationCoverage {
    member_count: u64,
    sample_count: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanLocator {
    codec: String,
    codec_version: u32,
    payload: String,
}
impl DatasetObservation {
    fn topic(self) -> Result<DatasetTopic, String> {
        let mut variant = String::new();
        let name = if self.scan_locator.codec == "spiral.mcap.topic-scan"
            && self.scan_locator.codec_version == 1
        {
            let locator: McapTopicLocator = serde_json::from_str(&self.scan_locator.payload)
                .map_err(|error| format!("Reading MCAP topic identity: {error}"))?;
            variant = locator.variant;
            locator.topic
        } else {
            self.name
        };
        let schemas = self
            .variants
            .into_iter()
            .filter_map(|variant| variant.native_schema_name)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(DatasetTopic::new(name, schemas)
            .with_revision(spiral_robotics_common::datasets::TopicRevision::new(
                variant,
            ))
            .with_coverage(self.coverage.member_count, self.coverage.sample_count))
    }
}
#[derive(Deserialize)]
struct McapTopicLocator {
    topic: String,
    #[serde(default)]
    variant: String,
}

pub(crate) struct EpisodeTopicQuery {
    statement: crate::db::SqlStatement,
}

impl EpisodeTopicQuery {
    pub(crate) fn for_episode(
        dataset: &DatasetSelection,
        episode_id: &EpisodeId,
        sql: String,
    ) -> Result<Self, String> {
        let selection = DatasetSelection::new(dataset.path().to_vec())?;
        if !selection.can_preview() {
            return Err("Preview is unavailable for this dataset name".to_owned());
        }
        let id = uuid::Uuid::parse_str(episode_id.as_str())
            .map_err(|_| "Invalid recording UUID".to_owned())?;
        let values = arrow_array::FixedSizeBinaryArray::try_from_iter([id.as_bytes()].into_iter())
            .map_err(|error| error.to_string())?;
        let field =
            arrow_schema::Field::new("id", arrow_schema::DataType::FixedSizeBinary(16), false)
                .with_extension_type(arrow_schema::extension::Uuid);
        let parameters = arrow_array::RecordBatch::try_new(
            std::sync::Arc::new(arrow_schema::Schema::new(vec![field])),
            vec![std::sync::Arc::new(values)],
        )
        .map_err(|error| error.to_string())?;
        crate::db::SqlStatement::from(sql)
            .with_parameters(parameters)
            .map(|statement| Self { statement })
            .map_err(|error| error.to_string())
    }
    pub(crate) fn into_statement(self) -> crate::db::SqlStatement {
        self.statement
    }
}

impl StorageManager {
    pub async fn project_resources(&self) -> Result<Vec<ProjectResource>, String> {
        self.project_store().dataset_catalog().resources().await
    }

    pub async fn validate_mcap_destination(&self, name: &str) -> Result<bool, String> {
        self.project_store().validate_mcap_destination(name).await
    }

    pub async fn inspect_dataset(
        &self,
        selection: DatasetSelection,
    ) -> Result<DatasetInspection, String> {
        let selection = DatasetSelection::new(selection.path().to_vec())?;
        self.project_store()
            .dataset_catalog()
            .inspect(selection)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episode_reads_bind_uuid_bytes_and_reject_invalid_ids() {
        let dataset = DatasetSelection::new(vec!["dataset_default".into()]).unwrap();
        let id = uuid::Uuid::parse_str("01900000-0000-7000-8000-000000000001").unwrap();
        let statement = EpisodeTopicQuery::for_episode(
            &dataset,
            &EpisodeId::try_from(id.simple().to_string()).unwrap(),
            "SELECT value FROM source WHERE id = $1".into(),
        )
        .unwrap()
        .statement;
        assert!(statement.as_str().contains("WHERE id = $1"));
        assert!(!statement.as_str().contains(&id.to_string()));
        let parameters = statement.parameters().unwrap();
        assert_eq!(
            parameters
                .schema()
                .field(0)
                .metadata()
                .get("ARROW:extension:name")
                .unwrap(),
            "arrow.uuid"
        );
        let values = parameters
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::FixedSizeBinaryArray>()
            .unwrap();
        assert_eq!(values.value(0), id.as_bytes());
        // An id that is not a member UUID never reaches a statement at all.
        assert!(EpisodeId::try_from("' OR true --").is_err());
    }

    #[test]
    fn discovery_uses_typed_resources_and_namespace_ownership() {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(json!([
            {"parent":{"kind":"projectRoot"},"name":"recordings","resource":{"descriptor":{"resourceId":"parent","kind":"table"},"versionId":"v1"}},
            {"parent":{"kind":"resource","resourceId":"parent"},"name":"default","resource":{"descriptor":{"resourceId":"data","kind":"dataset"},"versionId":"v2"}},
            {"parent":{"kind":"projectRoot"},"name":"pretend.dataset","resource":{"descriptor":{"resourceId":"ordinary-table","kind":"table"},"versionId":"v3"}}
        ])).unwrap();
        let resources = DatasetNamespace::new(entries).resources().unwrap();
        assert_eq!(resources.len(), 3);
        let datasets = resources
            .iter()
            .filter_map(ProjectResource::dataset)
            .collect::<Vec<_>>();
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].selection().path(), &["recordings", "default"]);
        assert_eq!(datasets[0].resource_id(), "data");
        assert_eq!(datasets[0].version_id(), "v2");
    }
    #[test]
    fn discovery_resolves_prefixes_from_project_roots_and_resource_anchors() {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(json!([
            {"parent":{"kind":"prefix","resourceId":"recordings","path":["camera","left"]},"name":"frames","resource":{"descriptor":{"resourceId":"frames","kind":"dataset"},"versionId":"v2"}},
            {"parent":{"kind":"resource","resourceId":"frames"},"name":"scores","resource":{"descriptor":{"resourceId":"scores","kind":"table"},"versionId":"v3"}},
            {"parent":{"kind":"prefix","path":["experiments","day1"]},"name":"recordings","resource":{"descriptor":{"resourceId":"recordings","kind":"dataset"},"versionId":"v1"}},
            {"parent":{"kind":"prefix","resourceId":null,"path":["archive"]},"name":"recordings","resource":{"descriptor":{"resourceId":"archive","kind":"dataset"},"versionId":"v4"}}
        ])).unwrap();
        let resources = DatasetNamespace::new(entries).resources().unwrap();
        assert_eq!(
            resources
                .iter()
                .map(|resource| resource.selection().label())
                .collect::<Vec<_>>(),
            [
                "archive.recordings",
                "experiments.day1.recordings",
                "experiments.day1.recordings.camera.left.frames",
                "experiments.day1.recordings.camera.left.frames.scores",
            ]
        );
        let dataset = resources[2].dataset().unwrap();
        assert_eq!(dataset.resource_id(), "frames");
        assert_eq!(dataset.version_id(), "v2");
    }

    #[test]
    fn discovery_counts_prefix_segments_toward_the_nesting_limit() {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(json!([
            {"parent":{"kind":"prefix","path":vec!["nested"; 32]},"name":"recordings","resource":{"descriptor":{"resourceId":"data","kind":"dataset"},"versionId":"v1"}}
        ])).unwrap();
        assert_eq!(
            DatasetNamespace::new(entries).resources().unwrap_err(),
            "Dataset namespace exceeds the nesting limit"
        );
    }

    #[test]
    fn mcap_topic_labels_come_from_the_versioned_native_locator() {
        let observation: DatasetObservation = serde_json::from_value(json!({
            "name":"mcap.opaque", "variants":[{"nativeSchemaName":"foxglove.CompressedVideo"}],
            "coverage":{"memberCount":52,"sampleCount":10937},
            "scanLocator":{"codec":"spiral.mcap.topic-scan","codecVersion":1,"payload":"{\"topic\":\"/camera/top/video\"}"}
        })).unwrap();
        let topic = observation.topic().unwrap();
        assert_eq!(topic.name(), "/camera/top/video");
        assert_eq!(topic.samples(), Some(10937));
        assert_eq!(topic.members(), 52);
    }
}
