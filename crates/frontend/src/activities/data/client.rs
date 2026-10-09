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
    /// Start importing the LeRobot dataset `source` names; the backend holds
    /// the run, which answers its status at once.
    pub(super) fn start_lerobot_import(
        &self,
        source: &spiral_robotics_common::datasets::HuggingFaceDatasetSource,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::datasets::LeRobotImportStatus, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/lerobot/imports").json(source))
    }
    /// Where the import run `id` has got.
    pub(super) fn lerobot_import(
        &self,
        id: &str,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::datasets::LeRobotImportStatus, CommandError>,
    > + use<> {
        self.project.request(HttpRequest::new(
            "GET",
            format!("/api/datasets/lerobot/imports/{id}"),
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

    /// The joint roles resolved for the dataset's current version; `None`
    /// until they are.
    pub(in crate::activities) fn joint_roles(
        &self,
        selection: &DatasetSelection,
    ) -> impl Future<
        Output = Result<Option<spiral_robotics_common::datasets::DatasetJointRoles>, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/joint-roles").json(selection))
    }

    pub(super) async fn save_joint_roles(
        &self,
        save: &spiral_robotics_common::datasets::DatasetJointRolesSave,
    ) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/joint-roles/save").json(save))
            .await
    }

    /// The check report kept for the dataset's current version, from checks
    /// at the lookup's definition.
    pub(super) fn check_report(
        &self,
        lookup: &spiral_robotics_common::datasets::DatasetCheckLookup,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/checks").json(lookup))
    }

    pub(super) async fn save_check_report(
        &self,
        save: &spiral_robotics_common::datasets::DatasetCheckSave,
    ) -> Result<(), CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/datasets/checks/save").json(save))
            .await
    }

    pub(crate) fn project_resources(
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
