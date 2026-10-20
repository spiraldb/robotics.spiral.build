//! HTTP endpoints for datasets and queries.

use super::query::QueryCommand;
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, ArrowResponse, Project};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::datasets::{
    DatasetImportRequest, DatasetImportStatus, DatasetInspection, DatasetJointRoles,
    DatasetJointRolesSave, DatasetRobot, DatasetRobotSave, DatasetSelection, DatasetSummary,
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_common::relations::{
    EpisodeQueryRequest, EpisodesQueryRequest, QueryRequest, SpiralQuerySnapshot,
};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

pub(crate) struct DatasetQueryEndpoints;

impl DatasetQueryEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/data-catalog", get(Self::datasets))
            .route("/datasets/inspect", post(Self::inspect_dataset))
            .route("/datasets/robot", post(Self::dataset_robot))
            .route("/datasets/joint-roles", post(Self::dataset_joint_roles))
            .route("/queries", post(Self::run_spiral_query))
            .route("/queries/episode", post(Self::run_episode_query))
            .route(
                "/queries/episode/arrow",
                post(Self::run_episode_arrow_query),
            )
            .route("/queries/episodes", post(Self::run_episodes_query))
            .route(
                "/queries/episodes/arrow",
                post(Self::run_episodes_arrow_query),
            )
            .route(
                "/datasets/hugging-face/inspect",
                post(Self::inspect_hugging_face_dataset),
            )
            .route(
                "/datasets/lerobot/inspect",
                post(Self::inspect_lerobot_dataset),
            )
            .route("/datasets/imports/{id}", get(Self::import_status))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/datasets/robot/save", post(Self::save_dataset_robot))
            .route(
                "/datasets/joint-roles/save",
                post(Self::save_dataset_joint_roles),
            )
            .route("/datasets/imports", post(Self::start_import))
            .route("/datasets/delete", post(Self::delete_dataset))
    }
    /// Every dataset of the project, each at the version it is at now.
    async fn datasets(
        Project(project): Project,
    ) -> Result<Json<Vec<DatasetSummary>>, OperationError> {
        let datasets = project
            .storage()
            .datasets()
            .await
            .map_err(OperationError::External)?;
        Ok(Json(datasets))
    }
    async fn dataset_robot(
        Project(project): Project,
        ApiJson(dataset): ApiJson<DatasetSummary>,
    ) -> Result<Json<Option<DatasetRobot>>, OperationError> {
        let robot = project
            .storage()
            .dataset_robot(&dataset)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(robot))
    }
    async fn save_dataset_robot(
        Project(project): Project,
        ApiJson(save): ApiJson<DatasetRobotSave>,
    ) -> Result<Json<()>, OperationError> {
        project
            .storage()
            .save_dataset_robot(save)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(()))
    }
    async fn dataset_joint_roles(
        Project(project): Project,
        ApiJson(dataset): ApiJson<DatasetSummary>,
    ) -> Result<Json<Option<DatasetJointRoles>>, OperationError> {
        let roles = project
            .storage()
            .dataset_joint_roles(&dataset)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(roles))
    }
    async fn save_dataset_joint_roles(
        Project(project): Project,
        ApiJson(save): ApiJson<DatasetJointRolesSave>,
    ) -> Result<Json<()>, OperationError> {
        project
            .storage()
            .save_dataset_joint_roles(save)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(()))
    }
    /// Drop every table of the dataset; one already gone stays deleted.
    async fn delete_dataset(
        Project(project): Project,
        ApiJson(dataset): ApiJson<DatasetSelection>,
    ) -> Result<Json<()>, OperationError> {
        project
            .storage()
            .delete_dataset(&dataset)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(()))
    }
    async fn inspect_dataset(
        Project(project): Project,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<DatasetInspection>, OperationError> {
        let inspection = project
            .storage()
            .inspect_dataset(selection)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(inspection))
    }
    async fn run_spiral_query(
        Project(project): Project,
        ApiJson(request): ApiJson<QueryRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, OperationError> {
        let query = QueryCommand::admitted(request)?;
        let snapshot = project
            .storage()
            .queries()
            .query(query.statement(), query.max_rows())
            .await
            .map_err(OperationError::External)?;
        Ok(Json(snapshot))
    }
    async fn run_episode_query(
        Project(project): Project,
        ApiJson(request): ApiJson<EpisodeQueryRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, OperationError> {
        let request = QueryCommand::admitted_for_episode(request)?;
        let snapshot = project
            .storage()
            .queries()
            .query_for_episode(&request)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(snapshot))
    }
    async fn run_episode_arrow_query(
        Project(project): Project,
        ApiJson(request): ApiJson<EpisodeQueryRequest>,
    ) -> Result<ArrowResponse, OperationError> {
        let stream = project
            .storage()
            .queries()
            .arrow_for_episode(&request)
            .await
            .map_err(OperationError::External)?;
        Ok(ArrowResponse(stream))
    }
    async fn run_episodes_query(
        Project(project): Project,
        ApiJson(request): ApiJson<EpisodesQueryRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, OperationError> {
        let snapshot = project
            .storage()
            .queries()
            .query_for_episodes(&request)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(snapshot))
    }
    async fn run_episodes_arrow_query(
        Project(project): Project,
        ApiJson(request): ApiJson<EpisodesQueryRequest>,
    ) -> Result<ArrowResponse, OperationError> {
        let stream = project
            .storage()
            .queries()
            .arrow_for_episodes(&request)
            .await
            .map_err(OperationError::External)?;
        Ok(ArrowResponse(stream))
    }
    async fn inspect_hugging_face_dataset(
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<HuggingFaceDatasetInspection>, OperationError> {
        let inspection = HuggingFaceDatasetInspection::discover(request)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(inspection))
    }
    async fn inspect_lerobot_dataset(
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<HuggingFaceDatasetInspection>, OperationError> {
        let inspection = HuggingFaceDatasetInspection::discover_lerobot(request)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(inspection))
    }
    /// Start importing a Hugging Face repository as a dataset of the format
    /// `request` names; the run answers its status at once and is polled from
    /// then on.
    async fn start_import(
        Project(project): Project,
        ApiJson(request): ApiJson<DatasetImportRequest>,
    ) -> Result<Json<DatasetImportStatus>, OperationError> {
        Ok(Json(project.dataset_imports().start(request).await?))
    }
    async fn import_status(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<DatasetImportStatus>, OperationError> {
        Ok(Json(project.dataset_imports().status(&id)?))
    }
}
