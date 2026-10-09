//! HTTP endpoints for settings.

use crate::OperationError;
use crate::http::delivery::{ApiJson, Project, SignedIn};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post, put},
};
use spiral_robotics_common::settings::{
    ConnectionTest, DataSettings, PolicyRuntimeUpdate, SettingsUpdate, SpiralSettings,
};
use spiral_robotics_common::workspace::WorkspaceChoice;

pub(crate) struct SettingsEndpoints;

impl SettingsEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/settings/connection", get(Self::connection_settings))
            .route("/settings", get(Self::data_settings))
            .route("/settings/spiral/test", post(Self::test_spiral_connection))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/settings/connection", put(Self::choose_workspace))
            .route("/settings", put(Self::update_data_settings))
            .route("/settings/policy-runtime", put(Self::set_policy_runtime))
    }
    /// The endpoints, and the workspace this browser's session works in.
    async fn connection_settings(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
    ) -> Result<Json<SpiralSettings>, OperationError> {
        Ok(Json(state.projects().session_settings(&session).await?))
    }
    async fn choose_workspace(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
        ApiJson(request): ApiJson<WorkspaceChoice>,
    ) -> Result<Json<SpiralSettings>, OperationError> {
        Ok(Json(
            state
                .projects()
                .choose_workspace(&session, request.project_id())
                .await?,
        ))
    }
    async fn data_settings(
        Project(project): Project,
    ) -> Result<Json<DataSettings>, OperationError> {
        Ok(Json(project.projects().settings(project.project()).await?))
    }
    async fn update_data_settings(
        Project(project): Project,
        ApiJson(request): ApiJson<SettingsUpdate>,
    ) -> Result<Json<DataSettings>, OperationError> {
        Ok(Json(
            project
                .projects()
                .update_settings(project.project(), request)
                .await?,
        ))
    }
    async fn set_policy_runtime(
        Project(project): Project,
        ApiJson(request): ApiJson<PolicyRuntimeUpdate>,
    ) -> Result<Json<DataSettings>, OperationError> {
        Ok(Json(
            project
                .projects()
                .set_policy_runtime(project.project(), &request)
                .await?,
        ))
    }
    async fn test_spiral_connection(Project(project): Project) -> Json<ConnectionTest> {
        Json(ConnectionTest::from(
            project.storage().database().test().await,
        ))
    }
}
