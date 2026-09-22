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
    pub(super) fn robot(
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

    pub(super) async fn robot_discovery_context(
        &self,
        selection: &DatasetSelection,
    ) -> Result<spiral_robotics_common::datasets::RobotDiscoveryContext, CommandError> {
        self.project
            .request(
                HttpRequest::new("POST", "/api/datasets/robot-discovery/context").json(selection),
            )
            .await
    }

    pub(super) async fn read_robot_description(&self, url: &str) -> Result<String, CommandError> {
        let credentials = crate::client::proxy::ProxyClient::credentials().await?;
        let mut request = crate::client::proxy::ProxyClient::request("GET", url).header(
            "accept",
            "text/plain, application/json, application/xml, text/xml",
        );
        if url.starts_with("https://huggingface.co/")
            && let Some(credentials) = credentials.hugging_face()
        {
            request = request.header("authorization", format!("Bearer {}", credentials.token()));
        }
        crate::client::proxy::ProxyClient::text(request.timeout(30_000), 512 * 1024).await
    }

    pub(super) async fn inspect_robot_stream(
        &self,
        request: &spiral_robotics_common::datasets::RobotStreamInspectionRequest,
    ) -> Result<serde_json::Value, CommandError> {
        self.project
            .request(
                HttpRequest::new("POST", "/api/datasets/robot-discovery/inspect-stream")
                    .json(request),
            )
            .await
    }

    pub(super) async fn save_robot_discovery(
        &self,
        result: &spiral_robotics_common::datasets::RobotDiscoveryResult,
    ) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/robot-discovery/save").json(result))
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

    /// Recorded camera topics and schema details for one dataset.
    pub(in crate::activities) fn dataset_cameras(
        &self,
        selection: &DatasetSelection,
    ) -> impl Future<Output = Result<serde_json::Value, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/cameras").json(selection))
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

    /// Message density for every stream of one episode, in request order.
    pub(super) fn episode_coverage(
        &self,
        request: spiral_robotics_common::datasets::EpisodeCoverageRequest,
    ) -> impl Future<
        Output = Result<Vec<spiral_robotics_common::datasets::StreamCoverage>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/episode-coverage").json(&request))
    }

    pub(super) fn episode_messages(
        &self,
        request: spiral_robotics_common::datasets::MessageRead,
    ) -> impl Future<Output = Result<Vec<u8>, CommandError>> + use<> {
        self.project.request_bytes(
            HttpRequest::new("POST", "/api/datasets/episode-messages").json(&request),
        )
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
}
