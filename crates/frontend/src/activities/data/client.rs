//! Dataset requests and their query invalidation policy.
use super::inspections::{DatasetInspections, Inspecting};
use crate::client::project::ProjectClient;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::datasets::{
    DatasetImportRequest, DatasetImportStatus, DatasetInspection, DatasetSelection, DatasetSummary,
};
use spiral_robotics_common::relations::{
    EpisodeQueryRequest, EpisodesQueryRequest, QueryRequest, SpiralQuerySnapshot,
};
use std::future::Future;

#[derive(Clone)]
pub(crate) struct DatasetClient {
    project: ProjectClient,
    inspections: DatasetInspections,
}
impl DatasetClient {
    /// The robot saved for `dataset`'s version.
    pub(in crate::activities) fn robot(
        &self,
        dataset: &DatasetSummary,
    ) -> impl Future<
        Output = Result<Option<spiral_robotics_common::datasets::DatasetRobot>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/robot").json(dataset))
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

    /// What importing the LeRobot dataset `source` names would read, at the
    /// commit its revision resolves to now.
    pub(super) fn inspect_lerobot(
        &self,
        source: &spiral_robotics_common::datasets::HuggingFaceDatasetSource,
    ) -> impl Future<
        Output = Result<
            spiral_robotics_common::datasets::HuggingFaceDatasetInspection,
            CommandError,
        >,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/lerobot/inspect").json(source))
    }
    /// Start the import `request` asks for; the backend holds the run, which
    /// answers its status at once.
    pub(super) fn start_import(
        &self,
        request: &DatasetImportRequest,
    ) -> impl Future<Output = Result<DatasetImportStatus, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/imports").json(request))
    }
    /// Drop every table of `dataset`: its episodes, recordings and answers.
    pub(super) fn delete_dataset(
        &self,
        dataset: &DatasetSelection,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/delete").json(dataset))
    }
    /// Where the import run `id` has got.
    pub(super) fn import_status(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<DatasetImportStatus, CommandError>> + use<> {
        self.project.request(HttpRequest::new(
            "GET",
            format!("/api/datasets/imports/{id}"),
        ))
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

    /// The joint roles resolved for `dataset`'s version; `None` until they
    /// are.
    pub(in crate::activities) fn joint_roles(
        &self,
        dataset: &DatasetSummary,
    ) -> impl Future<
        Output = Result<Option<spiral_robotics_common::datasets::DatasetJointRoles>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/joint-roles").json(dataset))
    }

    pub(super) async fn save_joint_roles(
        &self,
        save: &spiral_robotics_common::datasets::DatasetJointRolesSave,
    ) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/joint-roles/save").json(save))
            .await
    }

    /// Every dataset of the project, each at the version it is at now.
    pub(crate) fn datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<DatasetSummary>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/data-catalog"))
    }
    /// `selection`'s inspection if the project has read it already.
    pub(in crate::activities) fn inspected(
        &self,
        selection: &DatasetSelection,
    ) -> Option<DatasetInspection> {
        self.inspections.known(selection)
    }
    /// `selection`'s inspection, read once for every surface that asks.
    pub(in crate::activities) fn inspect_dataset(&self, selection: DatasetSelection) -> Inspecting {
        let project = self.project.clone();
        let dataset = selection.clone();
        self.inspections.inspect(&dataset, move || {
            project.request(HttpRequest::new("POST", "/api/datasets/inspect").json(&selection))
        })
    }
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
            inspections: DatasetInspections::current(),
        }
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
    /// A read-only query over one exact episode; `$1` binds its number.
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
    /// A read-only query over listed episodes of one dataset; `$1` through
    /// `$N` bind their numbers.
    pub(in crate::activities) fn run_episodes_query(
        &self,
        request: &EpisodesQueryRequest,
    ) -> impl Future<Output = Result<SpiralQuerySnapshot, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/queries/episodes").json(request))
    }
    /// The same listed episodes' query as one complete Arrow IPC stream,
    /// refused rather than cut short.
    pub(in crate::activities) fn run_episodes_arrow_query(
        &self,
        request: &EpisodesQueryRequest,
    ) -> impl Future<Output = Result<Vec<u8>, CommandError>> + use<> {
        self.project
            .request_bytes(HttpRequest::new("POST", "/api/queries/episodes/arrow").json(request))
    }
}
