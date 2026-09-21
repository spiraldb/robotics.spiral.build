//! HTTP endpoints for queries.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetSummary, ProjectResource,
};
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_common::relations::{QueryRequest, RelationCatalog, SpiralQuerySnapshot};

pub(crate) struct DatasetQueryEndpoints;

impl DatasetQueryEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/datasets", Self::datasets);
        routes.get("/data-catalog", Self::project_resources);
        routes.post("/datasets/inspect", Self::inspect_dataset);
        routes.post("/datasets/robot", Self::dataset_robot);
        routes.post(
            "/datasets/robot-discovery/context",
            Self::robot_discovery_context,
        );
        routes.post(
            "/datasets/robot-discovery/inspect-stream",
            Self::inspect_robot_stream,
        );
        routes.post("/datasets/robot-discovery/save", Self::save_robot_discovery);
        routes.post("/datasets/episode-messages", Self::episode_messages);
        routes.post("/datasets/episode-topics", Self::episode_topics);
        routes.post("/datasets/episode-coverage", Self::episode_coverage);
        routes.get("/relations", Self::relation_catalog);
        routes.post("/queries", Self::run_spiral_query);
        routes.post("/queries/episode", Self::run_episode_query);
        routes.post(
            "/datasets/hugging-face/inspect",
            Self::inspect_hugging_face_dataset,
        );
        routes.post(
            "/datasets/hugging-face/import",
            Self::import_hugging_face_dataset,
        );
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
    async fn robot_discovery_context(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(selection): ApiJson<DatasetSelection>,
    ) -> Result<Json<spiral_robotics_common::datasets::RobotDiscoveryContext>, HttpFailure> {
        Ok(Json(commands.robot_discovery_context(selection).await?))
    }
    async fn inspect_robot_stream(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::RobotStreamInspectionRequest>,
    ) -> Result<Json<serde_json::Value>, HttpFailure> {
        Ok(Json(commands.inspect_robot_stream(request).await?))
    }
    async fn save_robot_discovery(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(result): ApiJson<spiral_robotics_common::datasets::RobotDiscoveryResult>,
    ) -> Result<Json<()>, HttpFailure> {
        Ok(Json(commands.save_robot_discovery(result).await?))
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

    async fn episode_messages(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::MessageRead>,
    ) -> Result<crate::http::contract::ArrowResponse, HttpFailure> {
        Ok(crate::http::contract::ArrowResponse(
            commands.episode_messages(request).await?,
        ))
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
