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
        routes.post("/datasets/episode-topic", Self::preview_episode_topic);
        routes.post("/datasets/episode-video", Self::episode_video);
        routes.get("/relations", Self::relation_catalog);
        routes.post("/queries", Self::run_spiral_query);
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
    async fn preview_episode_topic(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::EpisodeTopicRequest>,
    ) -> Result<Json<SpiralQuerySnapshot>, HttpFailure> {
        Ok(Json(commands.preview_episode_topic(request).await?))
    }
    async fn episode_video(
        ScopedProject(commands): ScopedProject<crate::data::commands::DataCommands>,
        ApiJson(request): ApiJson<spiral_robotics_common::datasets::EpisodeTopicRequest>,
    ) -> Result<Json<spiral_robotics_common::datasets::EpisodeVideo>, HttpFailure> {
        Ok(Json(commands.episode_video(request).await?))
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
