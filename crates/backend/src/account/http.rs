//! HTTP endpoints for the Spiral account.

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post, put},
};
use spiral_robotics_common::account::{
    OrganizationSwitch, SpiralAccountStatus, SpiralOrganization, SpiralProject,
};

use crate::OperationError;
use crate::http::delivery::{ApiJson, Project};
use crate::projects::context::StudioContext;

pub(crate) struct AccountEndpoints;

impl AccountEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/account", get(Self::status))
            .route("/account/organizations", get(Self::organizations))
            .route("/account/projects", get(Self::projects))
    }

    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/account/sign-in", post(Self::sign_in))
            .route("/account/sign-out", post(Self::sign_out))
            .route("/account/organization", put(Self::switch_organization))
    }

    async fn status(State(studio): State<StudioContext>) -> Json<SpiralAccountStatus> {
        Json(studio.account().status().await)
    }

    async fn organizations(
        State(studio): State<StudioContext>,
    ) -> Result<Json<Vec<SpiralOrganization>>, OperationError> {
        let endpoint = studio.projects().saved_connection().endpoint().to_owned();
        Ok(Json(studio.account().organizations(&endpoint).await?))
    }

    async fn projects(
        State(studio): State<StudioContext>,
    ) -> Result<Json<Vec<SpiralProject>>, OperationError> {
        let endpoint = studio.projects().saved_connection().endpoint().to_owned();
        Ok(Json(studio.account().projects(&endpoint).await?))
    }

    async fn sign_in(
        State(studio): State<StudioContext>,
    ) -> Result<Json<SpiralAccountStatus>, OperationError> {
        let endpoint = studio.projects().saved_connection().endpoint().to_owned();
        Ok(Json(studio.account().sign_in(endpoint).await?))
    }

    /// Projects opened as this user close with the session.
    async fn sign_out(
        Project(project): Project,
    ) -> Result<Json<SpiralAccountStatus>, OperationError> {
        let account = project.account().clone();
        let status = project
            .projects()
            .change_account(project.project(), account.sign_out())
            .await?;
        Ok(Json(status))
    }

    /// Another organization has other projects, so the open ones close.
    async fn switch_organization(
        Project(project): Project,
        ApiJson(request): ApiJson<OrganizationSwitch>,
    ) -> Result<Json<SpiralAccountStatus>, OperationError> {
        let account = project.account().clone();
        let endpoint = project.projects().saved_connection().endpoint().to_owned();
        let switch = account.switch_organization(&endpoint, request.organization_id());
        let status = project
            .projects()
            .change_account(project.project(), switch)
            .await?;
        Ok(Json(status))
    }
}
