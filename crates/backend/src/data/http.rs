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
    DatasetInspection, DatasetJointRoles, DatasetJointRolesSave, DatasetRobot, DatasetRobotSave,
    DatasetSelection, DatasetSummary, DatasetTopic, EpisodeSelection,
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    LeRobotImportStatus, ProjectResource,
};
use spiral_robotics_common::relations::{
    EpisodeQueryRequest, EpisodesQueryRequest, QueryRequest, SpiralQuerySnapshot,
};
use spiral_robotics_storage::ingestion::HuggingFaceDatasetDiscovery as _;

pub(crate) struct DatasetQueryEndpoints;

impl DatasetQueryEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/data-catalog", get(Self::project_resources))
            .route("/datasets/inspect", post(Self::inspect_dataset))
            .route("/datasets/robot", post(Self::dataset_robot))
            .route("/datasets/joint-roles", post(Self::dataset_joint_roles))
            .route("/datasets/episode-topics", post(Self::episode_topics))
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
            .route(
                "/datasets/lerobot/imports/{id}",
                get(Self::lerobot_import_status),
            )
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/datasets/robot/save", post(Self::save_dataset_robot))
            .route(
                "/datasets/joint-roles/save",
                post(Self::save_dataset_joint_roles),
            )
            .route(
                "/datasets/hugging-face/import",
                post(Self::import_hugging_face_dataset),
            )
            .route(
                "/datasets/lerobot/imports",
                post(Self::import_lerobot_dataset),
            )
    }
    async fn project_resources(
        Project(project): Project,
    ) -> Result<Json<Vec<ProjectResource>>, OperationError> {
        let resources = project
            .storage()
            .project_resources()
            .await
            .map_err(OperationError::External)?;
        Ok(Json(resources))
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
    async fn episode_topics(
        Project(project): Project,
        ApiJson(request): ApiJson<EpisodeSelection>,
    ) -> Result<Json<Vec<DatasetTopic>>, OperationError> {
        let topics = project
            .storage()
            .episode_topics(request)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(topics))
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
    /// Start importing a LeRobot dataset; the run answers its status at once
    /// and is polled from then on.
    async fn import_lerobot_dataset(
        Project(project): Project,
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<LeRobotImportStatus>, OperationError> {
        Ok(Json(project.lerobot_imports().start(request).await?))
    }
    async fn lerobot_import_status(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<LeRobotImportStatus>, OperationError> {
        Ok(Json(project.lerobot_imports().status(&id)?))
    }
    async fn import_hugging_face_dataset(
        Project(project): Project,
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<HuggingFaceDatasetImportSummary>, OperationError> {
        let inspection = HuggingFaceDatasetInspection::discover(request)
            .await
            .map_err(OperationError::External)?;
        let summary = project
            .storage()
            .import_huggingface_dataset(inspection)
            .await
            .map_err(OperationError::External)?;
        Ok(Json(summary))
    }
}
