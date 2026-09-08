//! Native Dataset discovery through the same project journal used by the ADBC driver.
use crate::{StorageManager, settings::SpiralConnectionSettings};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::json;
use spiral_robotics_common::native_datasets::{
    NativeDatasetInspection, NativeDatasetSelection, NativeDatasetSummary, NativeDatasetTopic,
};
use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

struct DatasetCatalogClient {
    client: reqwest::Client,
    endpoint: String,
    project: String,
}
impl DatasetCatalogClient {
    async fn connect(settings: SpiralConnectionSettings) -> Result<Self, String> {
        let project = settings
            .project_id()
            .ok_or("Select a Spiral project")?
            .to_owned();
        let endpoint = settings.endpoint().trim_end_matches('/').to_owned();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let mut headers = HeaderMap::new();
        if let Some(credentials) = settings.credentials() {
            let response = client.post(format!("{endpoint}/v1/oauth2/token")).json(&json!({
                "grant_type": "client_credentials", "client_id": credentials.client_id(), "client_secret": credentials.client_secret()
            })).send().await.map_err(|error| format!("Authenticating the dataset catalog: {error}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "Dataset catalog authentication failed ({})",
                    response.status()
                ));
            }
            let token: AccessToken = response
                .json()
                .await
                .map_err(|error| format!("Reading dataset catalog authentication: {error}"))?;
            let mut authorization =
                HeaderValue::from_str(&format!("Bearer {}", token.access_token))
                    .map_err(|_| "Invalid dataset catalog authentication header")?;
            authorization.set_sensitive(true);
            headers.insert(AUTHORIZATION, authorization);
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            endpoint,
            project,
        })
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, String> {
        let response = request
            .send()
            .await
            .map_err(|error| format!("Reading Spiral dataset metadata: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Spiral dataset metadata request failed ({})",
                response.status()
            ));
        }
        let mut response = response;
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

    async fn datasets(&self) -> Result<Vec<NativeDatasetSummary>, String> {
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
                return DatasetNamespace::new(namespace).datasets();
            }
        }
        Err("Spiral project catalog exceeds the inspection limit".to_owned())
    }

    async fn inspect(
        &self,
        selection: NativeDatasetSelection,
    ) -> Result<NativeDatasetInspection, String> {
        // Resolve membership in this project again; client-supplied IDs never select another project.
        let dataset = self
            .datasets()
            .await?
            .into_iter()
            .find(|dataset| dataset.selection() == &selection)
            .ok_or("Dataset no longer exists in this project; refresh the catalog")?;
        let response: DatasetVersionResponse = self
            .request(self.client.get(format!(
                "{}/datasets/{}/versions/{}",
                self.endpoint,
                dataset.resource_id(),
                dataset.version_id()
            )))
            .await?;
        let Some(revision) = response.version.data_revision else {
            return Ok(NativeDatasetInspection::new(dataset, None, vec![]));
        };
        let mut topics = revision
            .data
            .schema
            .observations
            .into_iter()
            .map(|observation| observation.topic())
            .collect::<Result<Vec<_>, _>>()?;
        topics.sort_by(|left, right| left.name().cmp(right.name()));
        Ok(NativeDatasetInspection::new(
            dataset,
            Some(revision.data.member_count),
            topics,
        ))
    }
}

#[derive(Deserialize)]
struct AccessToken {
    access_token: String,
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
    Resource { resource_id: String },
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
    fn datasets(self) -> Result<Vec<NativeDatasetSummary>, String> {
        let mut queue = VecDeque::from([(None::<String>, Vec::<String>::new())]);
        let mut visited = BTreeSet::new();
        let mut datasets = Vec::new();
        while let Some((parent, prefix)) = queue.pop_front() {
            for entry in &self.entries {
                let matches = match &entry.parent {
                    NamespaceParent::ProjectRoot => parent.is_none(),
                    NamespaceParent::Resource { resource_id } => {
                        parent.as_ref() == Some(resource_id)
                    }
                };
                if !matches {
                    continue;
                }
                let mut path = prefix.clone();
                path.push(entry.name.clone());
                if path.len() > 32 {
                    return Err("Dataset namespace exceeds the nesting limit".to_owned());
                }
                let resource = &entry.resource;
                if resource.descriptor.kind == "dataset" {
                    datasets.push(NativeDatasetSummary::new(
                        NativeDatasetSelection::new(path.clone())?,
                        (
                            resource.descriptor.resource_id.clone(),
                            resource.version_id.clone(),
                        ),
                    ));
                }
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
    fn topic(self) -> Result<NativeDatasetTopic, String> {
        let name = if self.scan_locator.codec == "spiral.mcap.topic-scan"
            && self.scan_locator.codec_version == 1
        {
            let locator: McapTopicLocator = serde_json::from_str(&self.scan_locator.payload)
                .map_err(|error| format!("Reading MCAP topic identity: {error}"))?;
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
        Ok(NativeDatasetTopic::new(name, schemas)
            .with_coverage(self.coverage.member_count, self.coverage.sample_count))
    }
}
#[derive(Deserialize)]
struct McapTopicLocator {
    topic: String,
}

impl StorageManager {
    pub async fn native_datasets(&self) -> Result<Vec<NativeDatasetSummary>, String> {
        DatasetCatalogClient::connect(self.connection_settings())
            .await?
            .datasets()
            .await
    }
    pub async fn inspect_native_dataset(
        &self,
        selection: NativeDatasetSelection,
    ) -> Result<NativeDatasetInspection, String> {
        let selection = NativeDatasetSelection::new(selection.path().to_vec())?;
        DatasetCatalogClient::connect(self.connection_settings())
            .await?
            .inspect(selection)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_uses_typed_resources_and_namespace_ownership() {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(json!([
            {"parent":{"kind":"projectRoot"},"name":"recordings","resource":{"descriptor":{"resourceId":"parent","kind":"table"},"versionId":"v1"}},
            {"parent":{"kind":"resource","resourceId":"parent"},"name":"default","resource":{"descriptor":{"resourceId":"data","kind":"dataset"},"versionId":"v2"}},
            {"parent":{"kind":"projectRoot"},"name":"pretend.dataset","resource":{"descriptor":{"resourceId":"ordinary-table","kind":"table"},"versionId":"v3"}}
        ])).unwrap();
        let datasets = DatasetNamespace::new(entries).datasets().unwrap();
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].selection().path(), &["recordings", "default"]);
        assert_eq!(datasets[0].resource_id(), "data");
        assert_eq!(datasets[0].version_id(), "v2");
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
