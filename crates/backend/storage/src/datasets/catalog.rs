//! The project's Studio datasets as its journal names them, read through the
//! same journal the ADBC driver uses: every table under `datasets.<id>`,
//! grouped by dataset. A dataset exists when its `episodes` table does, and
//! is an MCAP dataset when it also has `files`, so listing datasets reads no
//! rows.
use crate::{SpiralAccess, SpiralConnection, StorageManager};
use serde::Deserialize;
use serde_json::json;
use spiral_robotics_common::datasets::{
    DatasetFormat, DatasetSelection, DatasetSummary, DatasetTable, TopicRevision,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
            .map_err(|error| format!("Reading the Spiral catalog: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Spiral catalog request failed ({})",
                response.status()
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err("The Spiral catalog exceeds the 16 MiB response limit".to_owned());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("Decoding the Spiral catalog: {error}"))
    }

    /// Every Studio dataset the project's journal holds now.
    pub(crate) async fn datasets(&self) -> Result<CatalogDatasets, String> {
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
            let page: JournalPage = self
                .request(self.client.post(format!("{prefix}/state/read")).json(
                    &json!({ "commitId": branch.head_commit_id, "pageSize": 1000, "pageToken": token }),
                ))
                .await?;
            namespace.extend(page.namespace);
            token = page.next_page_token;
            if token.is_none() {
                return Ok(CatalogDatasets::new(
                    DatasetNamespace::new(namespace).tables()?,
                ));
            }
        }
        Err("Spiral project catalog exceeds the inspection limit".to_owned())
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
    resource: NamespaceResource,
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
struct NamespaceResource {
    descriptor: ResourceDescriptor,
    version_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDescriptor {
    resource_id: String,
    kind: String,
}

/// One table the journal names: its full path, and its resource and
/// version.
struct CatalogTable {
    path: Vec<String>,
    identity: (String, String),
}

struct DatasetNamespace {
    entries: Vec<NamespaceEntry>,
}
impl DatasetNamespace {
    const TABLE: &str = "table";

    fn new(entries: Vec<NamespaceEntry>) -> Self {
        Self { entries }
    }

    /// Every table the namespace holds, each at its full path: entries are
    /// named relative to their parent resource, walked from the project root.
    fn tables(self) -> Result<Vec<CatalogTable>, String> {
        let mut queue = VecDeque::from([(None::<String>, Vec::<String>::new())]);
        let mut visited = BTreeSet::new();
        let mut tables = Vec::new();
        while let Some((parent, prefix)) = queue.pop_front() {
            for entry in &self.entries {
                if entry.parent.resource_id() != parent.as_deref() {
                    continue;
                }
                let mut path = prefix.clone();
                path.extend_from_slice(entry.parent.path());
                path.push(entry.name.clone());
                if path.len() > 32 {
                    return Err("The Spiral catalog exceeds the nesting limit".to_owned());
                }
                let resource = &entry.resource;
                if resource.descriptor.kind == Self::TABLE {
                    tables.push(CatalogTable {
                        path: path.clone(),
                        identity: (
                            resource.descriptor.resource_id.clone(),
                            resource.version_id.clone(),
                        ),
                    });
                }
                if visited.insert(resource.descriptor.resource_id.clone()) {
                    queue.push_back((Some(resource.descriptor.resource_id.clone()), path));
                }
            }
        }
        Ok(tables)
    }
}

/// The Studio datasets a catalog holds, each with every table under its
/// namespace.
pub(crate) struct CatalogDatasets {
    datasets: BTreeMap<DatasetSelection, DatasetTables>,
}

/// The tables under one dataset's namespace, by their path under it.
#[derive(Default)]
struct DatasetTables {
    tables: BTreeMap<Vec<String>, (String, String)>,
}

impl DatasetTables {
    fn identity(&self, table: &str) -> Option<&(String, String)> {
        self.tables.get([table.to_owned()].as_slice())
    }
}

impl CatalogDatasets {
    fn new(tables: Vec<CatalogTable>) -> Self {
        let mut datasets = BTreeMap::<DatasetSelection, DatasetTables>::new();
        for table in tables {
            if let Some((dataset, rest)) = DatasetSelection::of_catalog_path(&table.path) {
                datasets
                    .entry(dataset)
                    .or_default()
                    .tables
                    .insert(rest.to_vec(), table.identity);
            }
        }
        Self { datasets }
    }

    /// Every dataset, by name: each namespace holding an `episodes` table.
    pub(crate) fn summaries(&self) -> Vec<DatasetSummary> {
        self.datasets
            .keys()
            .filter_map(|dataset| self.summary(dataset))
            .collect()
    }

    /// `dataset`, when its `episodes` table exists: its format, and that
    /// table's resource and version as its identity.
    pub(crate) fn summary(&self, dataset: &DatasetSelection) -> Option<DatasetSummary> {
        let tables = self.datasets.get(dataset)?;
        let identity = tables.identity("episodes")?;
        let format = if tables.identity("files").is_some() {
            DatasetFormat::Mcap
        } else {
            DatasetFormat::LeRobot
        };
        Some(DatasetSummary::new(
            dataset.clone(),
            format,
            identity.clone(),
        ))
    }

    /// Every MCAP dataset, by name.
    pub(crate) fn mcap(&self) -> Vec<DatasetSelection> {
        self.summaries()
            .into_iter()
            .filter(|dataset| dataset.format() == DatasetFormat::Mcap)
            .map(|dataset| dataset.selection().clone())
            .collect()
    }

    /// Whether any table lives under `dataset`'s namespace.
    pub(crate) fn holds(&self, dataset: &DatasetSelection) -> bool {
        self.datasets.contains_key(dataset)
    }

    /// Every table under `dataset`'s namespace, as statements name it.
    pub(crate) fn tables(&self, dataset: &DatasetSelection) -> Result<Vec<DatasetTable>, String> {
        let Some(tables) = self.datasets.get(dataset) else {
            return Ok(Vec::new());
        };
        tables
            .tables
            .keys()
            .map(|path| match path.as_slice() {
                [table] if table == "episodes" => Ok(dataset.episodes()),
                [table] if table == "topics" => Ok(dataset.topics()),
                [table] if table == "files" => Ok(dataset.files()),
                [table] if table == "answers" => Ok(dataset.answers()),
                [messages, stream] if messages == "messages" => {
                    Ok(dataset.messages(&TopicRevision::new(stream.clone())?))
                }
                [scratch] => dataset.scratch(scratch),
                other => Err(format!(
                    "{}.{} is no table of Studio's",
                    dataset.label(),
                    other.join(".")
                )),
            })
            .collect()
    }
}

impl StorageManager {
    /// Every dataset of the project, as its catalog names it now.
    pub async fn datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        Ok(self
            .project_store()
            .dataset_catalog()
            .datasets()
            .await?
            .summaries())
    }
}

#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod test_journal;
#[cfg(test)]
pub(crate) use test_journal::TestJournal;

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace(entries: serde_json::Value) -> CatalogDatasets {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(entries).unwrap();
        CatalogDatasets::new(DatasetNamespace::new(entries).tables().unwrap())
    }

    fn table(parent: serde_json::Value, name: &str, id: &str) -> serde_json::Value {
        json!({"parent": parent, "name": name,
            "resource": {"descriptor": {"resourceId": id, "kind": "table"}, "versionId": format!("{id}-v")}})
    }

    #[test]
    fn a_dataset_is_the_tables_under_its_namespace_and_named_by_its_episodes() {
        let prefix = |id: &str| json!({"kind": "prefix", "path": ["datasets", id]});
        let datasets = namespace(json!([
            table(prefix("pick-place"), "episodes", "episodes-1"),
            table(prefix("pick-place"), "topics", "topics-1"),
            table(prefix("pick-place"), "files", "files-1"),
            table(
                json!({"kind": "prefix", "path": ["datasets", "pick-place", "messages"]}),
                "top_0a1b2c3d",
                "stream-1"
            ),
            table(prefix("svla"), "episodes", "episodes-2"),
            table(prefix("svla"), "answers", "answers-2"),
            // A namespace whose episodes are gone is no dataset, but its
            // tables are still there.
            table(prefix("leftover"), "topics", "topics-3"),
            table(json!({"kind": "projectRoot"}), "dataset_default", "old"),
            json!({"parent": {"kind": "projectRoot"}, "name": "datasets.view",
                "resource": {"descriptor": {"resourceId": "view", "kind": "view"}, "versionId": "v"}}),
        ]));
        let summaries = datasets.summaries();
        assert_eq!(
            summaries
                .iter()
                .map(|summary| (summary.selection().id(), summary.format()))
                .collect::<Vec<_>>(),
            [
                ("pick-place", DatasetFormat::Mcap),
                ("svla", DatasetFormat::LeRobot)
            ]
        );
        assert_eq!(summaries[0].identity(), ("episodes-1", "episodes-1-v"));
        let leftover = DatasetSelection::new("leftover").unwrap();
        assert!(datasets.holds(&leftover) && datasets.summary(&leftover).is_none());
        assert!(!datasets.holds(&DatasetSelection::new("default").unwrap()));
        assert_eq!(
            datasets
                .tables(&summaries[0].selection().clone())
                .unwrap()
                .iter()
                .map(DatasetTable::target)
                .collect::<Vec<_>>(),
            [
                "datasets.pick-place.episodes",
                "datasets.pick-place.files",
                "datasets.pick-place.messages.top_0a1b2c3d",
                "datasets.pick-place.topics",
            ]
        );
        assert_eq!(datasets.mcap(), [summaries[0].selection().clone()]);
    }

    #[test]
    fn tables_are_found_under_prefixes_and_resource_anchors() {
        let datasets = namespace(json!([
            json!({"parent": {"kind": "projectRoot"}, "name": "datasets",
                "resource": {"descriptor": {"resourceId": "namespace", "kind": "namespace"}, "versionId": "v"}}),
            table(
                json!({"kind": "prefix", "resourceId": "namespace", "path": ["default"]}),
                "episodes",
                "episodes"
            ),
            table(
                json!({"kind": "resource", "resourceId": "episodes"}),
                "answers",
                "nested"
            ),
        ]));
        let summaries = datasets.summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].selection().id(), "default");
        assert!(
            datasets.tables(summaries[0].selection()).is_err(),
            "episodes.answers is no table of Studio's"
        );
    }

    #[test]
    fn prefix_segments_count_toward_the_nesting_limit() {
        let entries: Vec<NamespaceEntry> = serde_json::from_value(json!([table(
            json!({"kind": "prefix", "path": vec!["nested"; 32]}),
            "episodes",
            "data"
        )]))
        .unwrap();
        assert_eq!(
            DatasetNamespace::new(entries).tables().err().unwrap(),
            "The Spiral catalog exceeds the nesting limit"
        );
    }
}
