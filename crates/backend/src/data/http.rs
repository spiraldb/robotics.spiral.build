//! HTTP endpoints for queries.

use crate::http::delivery::{ApiJson, ArrowResponse, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetSummary, ProjectResource,
};
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_common::relations::{QueryRequest, RelationCatalog, SpiralQuerySnapshot};

pub(crate) struct DatasetQueryEndpoints;

impl DatasetQueryEndpoints {
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route("/datasets", get(Self::datasets))
            .route("/data-catalog", get(Self::project_resources))
            .route("/datasets/inspect", post(Self::inspect_dataset))
            .route("/datasets/robot", post(Self::dataset_robot))
            .route("/datasets/robot/save", post(Self::save_dataset_robot))
            .route("/datasets/episode-topics", post(Self::episode_topics))
            .route("/datasets/episode-coverage", post(Self::episode_coverage))
            .route("/relations", get(Self::relation_catalog))
            .route("/queries", post(Self::run_spiral_query))
            .route("/queries/episode", post(Self::run_episode_query))
            .route(
                "/queries/episode/arrow",
                post(Self::run_episode_arrow_query),
            )
            .route(
                "/datasets/hugging-face/inspect",
                post(Self::inspect_hugging_face_dataset),
            )
            .route(
                "/datasets/hugging-face/import",
                post(Self::import_hugging_face_dataset),
            )
    }
    async fn project_resources(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<Vec<ProjectResource>>, HttpFailure> {
        Ok(Json(commands.project_resources().await?))
    }
    async fn dataset_robot(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<Option<spiral_robotics_common::datasets::DatasetRobot>>, HttpFailure> {
        Ok(Json(commands.dataset_robot(selection).await?))
    }
    async fn save_dataset_robot(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(save): ApiJson<spiral_robotics_common::datasets::DatasetRobotSave>,
    ) -> Result<Json<()>, HttpFailure> {
        Ok(Json(commands.save_dataset_robot(save).await?))
    }
    async fn datasets(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<Vec<DatasetSummary>>, HttpFailure> {
        Ok(Json(commands.datasets().await?))
    }
    async fn inspect_dataset(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<DatasetInspection>, HttpFailure> {
        Ok(Json(commands.inspect_dataset(selection).await?))
    }

    async fn episode_topics(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::EpisodeSelection>,
    ) -> Result<Json<Vec<spiral_robotics_common::datasets::DatasetTopic>>, HttpFailure> {
        Ok(Json(commands.episode_topics(request).await?))
    }

    async fn episode_coverage(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::EpisodeCoverageRequest>,
    ) -> Result<Json<Vec<spiral_robotics_common::datasets::StreamCoverage>>, HttpFailure> {
        Ok(Json(commands.episode_coverage(request).await?))
    }

    async fn relation_catalog(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
    ) -> Result<Json<RelationCatalog>, HttpFailure> {
        let result = commands.relations().await?;
        Ok(Json(result))
    }
    async fn run_spiral_query(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<QueryRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, HttpFailure> {
        let (statement, max_rows) = request.into_parts();
        let result = commands.query(statement, max_rows).await?;
        Ok(Json(result))
    }
    async fn run_episode_query(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::relations::EpisodeQueryRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, HttpFailure> {
        Ok(Json(commands.query_for_episode(request).await?))
    }
    async fn run_episode_arrow_query(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::relations::EpisodeQueryRequest>,
    ) -> Result<ArrowResponse, HttpFailure> {
        Ok(ArrowResponse(
            commands.query_for_episode_arrow(request).await?,
        ))
    }
    async fn inspect_hugging_face_dataset(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<HuggingFaceDatasetInspection>, HttpFailure> {
        let result = commands.inspect_hugging_face_dataset(request).await?;
        Ok(Json(result))
    }
    async fn import_hugging_face_dataset(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<HuggingFaceDatasetSource>,
    ) -> Result<Json<HuggingFaceDatasetImportSummary>, HttpFailure> {
        let result = commands.import_hugging_face_dataset(request).await?;
        Ok(Json(result))
    }
}
