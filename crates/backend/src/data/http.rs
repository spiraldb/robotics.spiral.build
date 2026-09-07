//! HTTP endpoints for queries.

use crate::http::contract::ApiRoutes;
use crate::http::delivery::{ApiJson, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_common::relations::{QueryRequest, RelationCatalog, SpiralQuerySnapshot};

pub(crate) struct DatasetQueryEndpoints;

impl DatasetQueryEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
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
