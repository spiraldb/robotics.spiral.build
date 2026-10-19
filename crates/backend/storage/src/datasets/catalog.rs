//! The project's Studio datasets as its journal names them: every table at
//! the project's root named `datasets.<id>.…`, grouped by dataset. A dataset
//! exists when its `episodes` table does, and is an MCAP dataset when it also
//! has `files`, so listing datasets reads no rows.
use crate::{SpiralAccess, SpiralConnection, StorageManager};
use serde::Deserialize;
use serde_json::json;
use spiral_robotics_common::curation::AnswerColumn;
use spiral_robotics_common::datasets::{
    DatasetFormat, DatasetSelection, DatasetSummary, DatasetTable, TopicRevision,
};
use std::collections::{BTreeMap, BTreeSet};
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
                    DatasetNamespace::new(namespace).tables(),
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
    /// Under another resource, which Studio never names a table under.
    Resource {},
    /// Implicit name segments relative to the project root or a resource.
    Prefix {
        resource_id: Option<String>,
        path: Vec<String>,
    },
}
impl NamespaceParent {
    /// Whether the entry is bound at the project's root.
    fn is_root(&self) -> bool {
        match self {
            Self::ProjectRoot => true,
            Self::Resource {} => false,
            Self::Prefix { resource_id, path } => resource_id.is_none() && path.is_empty(),
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

/// One table bound at the project's root: its name, and its resource and
/// version.
struct CatalogTable {
    name: String,
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

    /// Every table bound at the project's root, where Studio names its
    /// datasets' tables.
    fn tables(self) -> Vec<CatalogTable> {
        self.entries
            .into_iter()
            .filter(|entry| entry.parent.is_root() && entry.resource.descriptor.kind == Self::TABLE)
            .map(|entry| CatalogTable {
                name: entry.name,
                identity: (
                    entry.resource.descriptor.resource_id,
                    entry.resource.version_id,
                ),
            })
            .collect()
    }
}

/// The Studio datasets a catalog holds, each with every table its name
/// gives it.
pub(crate) struct CatalogDatasets {
    datasets: BTreeMap<DatasetSelection, DatasetTables>,
}

/// The tables of one dataset, by their names' parts after the dataset's.
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
            if let Some((dataset, rest)) = DatasetSelection::of_table_name(&table.name) {
                datasets
                    .entry(dataset)
                    .or_default()
                    .tables
                    .insert(rest, table.identity);
            }
        }
        Self { datasets }
    }

    /// Every dataset, by name: each one with an `episodes` table.
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

    /// Whether `dataset` has any table.
    pub(crate) fn holds(&self, dataset: &DatasetSelection) -> bool {
        self.datasets.contains_key(dataset)
    }

    /// Every table of `dataset`, as statements name it.
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
                [messages, stream] if messages == "messages" => {
                    Ok(dataset.messages(&TopicRevision::new(stream.clone())?))
                }
                [answers, column] if answers == "answers" => {
                    Ok(dataset.answers(&AnswerColumn::try_from(column.clone())?))
                }
                other => Err(format!(
                    "{}.{} is no table of Studio's",
                    dataset.label(),
                    other.join(".")
                )),
            })
            .collect()
    }

    /// The catalog name of every table under `dataset`, including any Studio
    /// no longer writes: everything deleting the dataset drops.
    fn table_names(&self, dataset: &DatasetSelection) -> Vec<String> {
        self.datasets.get(dataset).map_or_else(Vec::new, |tables| {
            tables
                .tables
                .keys()
                .map(|path| format!("{}.{}", dataset.label(), path.join(".")))
                .collect()
        })
    }

    /// Every answer column `dataset` holds a table for.
    pub(crate) fn answer_columns(
        &self,
        dataset: &DatasetSelection,
    ) -> Result<BTreeSet<AnswerColumn>, String> {
        let Some(tables) = self.datasets.get(dataset) else {
            return Ok(BTreeSet::new());
        };
        tables
            .tables
            .keys()
            .filter_map(|path| match path.as_slice() {
                [answers, column] if answers == "answers" => {
                    Some(AnswerColumn::try_from(column.clone()))
                }
                _ => None,
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

    /// Drop every table of `dataset` in one transaction, its answers and
    /// recordings with it; a dataset with no tables is already gone.
    pub async fn delete_dataset(&self, dataset: &DatasetSelection) -> Result<(), String> {
        let tables = self
            .project_store()
            .dataset_catalog()
            .datasets()
            .await?
            .table_names(dataset);
        if tables.is_empty() {
            return Ok(());
        }
        let mut connection = self.project_store().connect_exclusive().await?;
        connection
            .transaction("dataset delete", async |connection| {
                for table in &tables {
                    connection.drop_table(table).await?;
                }
                Ok(())
            })
            .await
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
        CatalogDatasets::new(DatasetNamespace::new(entries).tables())
    }

    fn table(name: &str, id: &str) -> serde_json::Value {
        json!({"parent": {"kind": "projectRoot"}, "name": name,
            "resource": {"descriptor": {"resourceId": id, "kind": "table"}, "versionId": format!("{id}-v")}})
    }

    #[test]
    fn a_dataset_is_the_tables_its_name_starts_and_named_by_its_episodes() {
        let datasets = namespace(json!([
            table("datasets.pick-place.episodes", "episodes-1"),
            table("datasets.pick-place.topics", "topics-1"),
            table("datasets.pick-place.files", "files-1"),
            table("datasets.pick-place.messages.top_0a1b2c3d", "stream-1"),
            table("datasets.svla.episodes", "episodes-2"),
            table("datasets.svla.answers.similarity_3f9a2c", "answers-2"),
            // A dataset whose episodes are gone is no dataset, but its
            // tables are still there.
            table("datasets.leftover.topics", "topics-3"),
            // A table Studio no longer writes still belongs to its dataset.
            table("datasets.leftover.answers", "answers-3"),
            table("dataset_default", "old"),
            json!({"parent": {"kind": "projectRoot"}, "name": "datasets.view.episodes",
                "resource": {"descriptor": {"resourceId": "view", "kind": "view"}, "versionId": "v"}}),
            // A table nested under a prefix is no Studio table.
            json!({"parent": {"kind": "prefix", "path": ["datasets", "nested"]}, "name": "episodes",
                "resource": {"descriptor": {"resourceId": "nested", "kind": "table"}, "versionId": "v"}}),
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
        assert!(!datasets.holds(&DatasetSelection::new("nested").unwrap()));
        assert_eq!(
            datasets
                .tables(&summaries[0].selection().clone())
                .unwrap()
                .iter()
                .map(|table| table.name().to_owned())
                .collect::<Vec<_>>(),
            [
                "datasets.pick-place.episodes",
                "datasets.pick-place.files",
                "datasets.pick-place.messages.top_0a1b2c3d",
                "datasets.pick-place.topics",
            ]
        );
        assert_eq!(datasets.mcap(), [summaries[0].selection().clone()]);
        let svla = summaries[1].selection();
        let similarity = AnswerColumn::try_from("similarity_3f9a2c".to_owned()).unwrap();
        assert_eq!(
            datasets.tables(svla).unwrap(),
            [svla.answers(&similarity), svla.episodes()]
        );
        assert_eq!(
            datasets.answer_columns(svla).unwrap(),
            BTreeSet::from([similarity])
        );
        assert!(datasets.answer_columns(&leftover).unwrap().is_empty());
        // Deleting a dataset drops every table under its name.
        assert_eq!(
            datasets.table_names(&leftover),
            ["datasets.leftover.answers", "datasets.leftover.topics"]
        );
        assert_eq!(
            datasets.table_names(svla),
            [
                "datasets.svla.answers.similarity_3f9a2c",
                "datasets.svla.episodes"
            ]
        );
        assert!(
            datasets
                .table_names(&DatasetSelection::new("default").unwrap())
                .is_empty()
        );
    }
}
