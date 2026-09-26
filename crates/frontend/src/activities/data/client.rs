//! Dataset requests and their query invalidation policy.
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection, ProjectResource};
use spiral_robotics_common::relations::{EpisodeQueryRequest, QueryRequest, SpiralQuerySnapshot};
use std::future::Future;

#[derive(Clone)]
pub(crate) struct DatasetClient {
    project: ProjectClient,
}
impl DatasetClient {
    pub(in crate::activities) fn robot(
        &self,
        selection: &DatasetSelection,
    ) -> impl Future<
        Output = Result<Option<spiral_robotics_common::datasets::DatasetRobot>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/robot").json(selection))
    }
    pub(super) fn inspect_import(
        &self,
        source: spiral_robotics_common::datasets::HuggingFaceDatasetSource,
    ) -> impl Future<
        Output = Result<
            spiral_robotics_common::datasets::HuggingFaceDatasetInspection,
            CommandError,
        >,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/hugging-face/inspect").json(&source))
    }
    pub(super) fn import_dataset(
        &self,
        source: spiral_robotics_common::datasets::HuggingFaceDatasetSource,
    ) -> impl Future<
        Output = Result<
            spiral_robotics_common::datasets::HuggingFaceDatasetImportSummary,
            CommandError,
        >,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/hugging-face/import").json(&source))
    }

    /// Robot files are public; the relay authenticates Hugging Face requests
    /// when a token is saved.
    pub(super) async fn read_robot_description(&self, url: &str) -> Result<String, CommandError> {
        let request = crate::client::proxy::ProxyClient::request("GET", url)
            .header(
                "accept",
                "text/plain, application/json, application/xml, text/xml",
            )
            .timeout(30_000);
        crate::client::proxy::ProxyClient::text(request, 512 * 1024).await
    }

    pub(super) async fn read_robot_asset(
        &self,
        url: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CommandError> {
        let request = crate::client::proxy::ProxyClient::request("GET", url).timeout(30_000);
        crate::client::proxy::ProxyClient::bytes(request, max_bytes).await
    }

    pub(super) async fn save_dataset_robot(
        &self,
        save: &spiral_robotics_common::datasets::DatasetRobotSave,
    ) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/robot/save").json(save))
            .await
    }

    pub(super) fn project_resources(
        &self,
    ) -> impl Future<Output = Result<Vec<ProjectResource>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/data-catalog"))
    }
    pub(in crate::activities) fn inspect_dataset(
        &self,
        selection: DatasetSelection,
    ) -> impl Future<Output = Result<DatasetInspection, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/inspect").json(&selection))
    }
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn list_datasets(
        &self,
    ) -> impl Future<
        Output = Result<Vec<spiral_robotics_common::datasets::DatasetSummary>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/datasets"))
    }

    pub(in crate::activities) fn episode_topics(
        &self,
        request: spiral_robotics_common::datasets::EpisodeSelection,
    ) -> impl Future<
        Output = Result<Vec<spiral_robotics_common::datasets::DatasetTopic>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/episode-topics").json(&request))
    }

    pub(in crate::activities) fn run_spiral_query(
        &self,
        statement: String,
        max_rows: usize,
    ) -> impl Future<Output = Result<SpiralQuerySnapshot, CommandError>> + use<> {
        self.project.request(
            HttpRequest::new("POST", "/api/queries").json(&QueryRequest::new(statement, max_rows)),
        )
    }
    /// A read-only query over one exact episode; `$1` binds its UUID.
    pub(in crate::activities) fn run_episode_query(
        &self,
        request: &EpisodeQueryRequest,
    ) -> impl Future<Output = Result<SpiralQuerySnapshot, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/queries/episode").json(request))
    }
    /// The same episode query as one complete Arrow IPC stream, refused
    /// rather than cut short; the statement bounds its own rows.
    pub(in crate::activities) fn run_episode_arrow_query(
        &self,
        request: &EpisodeQueryRequest,
    ) -> impl Future<Output = Result<Vec<u8>, CommandError>> + use<> {
        self.project
            .request_bytes(HttpRequest::new("POST", "/api/queries/episode/arrow").json(request))
    }
}
