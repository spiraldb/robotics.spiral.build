//! Dataset requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::native_datasets::{
    NativeDatasetInspection, NativeDatasetSelection, NativeDatasetSummary,
};
use spiral_robotics_common::recording::{
    DatasetSummary, EpisodeCatalog, EpisodeInspectionSnapshot,
};
use spiral_robotics_common::relations::{QueryRequest, RelationCatalog, SpiralQuerySnapshot};
use std::future::Future;

pub(crate) struct DatasetClient {
    project: ProjectClient,
}
impl DatasetClient {
    pub(crate) fn native_datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<NativeDatasetSummary>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/native-datasets"))
    }
    pub(crate) fn inspect_native_dataset(
        &self,
        selection: NativeDatasetSelection,
    ) -> impl Future<Output = Result<NativeDatasetInspection, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/native-datasets/inspect").json(&selection))
    }
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn list_datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<DatasetSummary>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/datasets"))
    }

    pub(crate) fn list_episodes(
        &self,
        dataset_id: String,
    ) -> impl Future<Output = Result<EpisodeCatalog, CommandError>> + use<> {
        self.project.request(HttpRequest::new(
            "GET",
            format!(
                "/api/datasets/{dataset_id}/episodes",
                dataset_id = dataset_id.url_component()
            ),
        ))
    }

    pub(crate) fn inspect_episode(
        &self,
        id: String,
    ) -> impl Future<Output = Result<EpisodeInspectionSnapshot, CommandError>> + use<> {
        self.project.request(HttpRequest::new(
            "GET",
            format!("/api/episodes/{id}", id = id.url_component()),
        ))
    }

    pub(crate) fn relation_catalog(
        &self,
    ) -> impl Future<Output = Result<RelationCatalog, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/relations"))
    }

    pub(crate) fn run_spiral_query(
        &self,
        statement: String,
        max_rows: usize,
    ) -> impl Future<Output = Result<SpiralQuerySnapshot, CommandError>> + use<> {
        self.project.request(
            HttpRequest::new("POST", "/api/queries").json(&QueryRequest::new(statement, max_rows)),
        )
    }
}
