//! HTTP endpoints for the signed-in Spiral user, and the gate every API
//! request passes: it must carry a live session.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::{AppendHeaders, IntoResponse, Response},
    routing::{get, post, put},
};
use spiral_robotics_common::account::{
    OrganizationSwitch, SpiralIdentity, SpiralOrganization, SpiralProject,
};

use super::SpiralSession;
use super::sign_in::{BrowserCookies, BrowserSignIn};
use crate::OperationError;
use crate::http::delivery::{ApiJson, Project, SignedIn};
use crate::projects::context::StudioContext;

pub(crate) struct AccountEndpoints;

impl AccountEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/account", get(Self::identity))
            .route("/account/organizations", get(Self::organizations))
            .route("/account/projects", get(Self::projects))
    }

    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/account/sign-out", post(Self::sign_out))
            .route("/account/organization", put(Self::switch_organization))
    }

    /// Admit a request only with a live session, which its handlers then read.
    pub(crate) async fn require_session(
        State(studio): State<StudioContext>,
        mut request: Request,
        next: Next,
    ) -> Response {
        let Some(id) = BrowserCookies::read(request.headers(), BrowserCookies::SESSION) else {
            return SpiralSession::signed_out().into_response();
        };
        match studio.sessions().find(id).await {
            Ok(session) => {
                request
                    .extensions_mut()
                    .insert::<Arc<SpiralSession>>(session);
                next.run(request).await
            }
            Err(error) => error.into_response(),
        }
    }

    async fn identity(SignedIn(session): SignedIn) -> Result<Json<SpiralIdentity>, OperationError> {
        Ok(Json(session.identity().await?))
    }

    async fn organizations(
        SignedIn(session): SignedIn,
    ) -> Result<Json<Vec<SpiralOrganization>>, OperationError> {
        Ok(Json(session.organizations().await?))
    }

    async fn projects(
        SignedIn(session): SignedIn,
    ) -> Result<Json<Vec<SpiralProject>>, OperationError> {
        Ok(Json(session.projects().await?))
    }

    /// This browser's projects close with its session, and its cookie goes.
    async fn sign_out(Project(project): Project) -> Result<Response, OperationError> {
        let session = project.session().clone();
        let sessions = project.studio().sessions().clone();
        project
            .projects()
            .change_account(&session, async { sessions.sign_out(&session).await })
            .await?;
        Ok((
            AppendHeaders([(
                header::SET_COOKIE,
                BrowserSignIn::signed_out(project.studio()),
            )]),
            Json(()),
        )
            .into_response())
    }

    /// Another organization has other projects, so this browser's open ones close.
    async fn switch_organization(
        Project(project): Project,
        ApiJson(request): ApiJson<OrganizationSwitch>,
    ) -> Result<Json<SpiralIdentity>, OperationError> {
        let session = project.session().clone();
        let identity = project
            .projects()
            .change_account(
                &session,
                session.switch_organization(request.organization_id()),
            )
            .await?;
        Ok(Json(identity))
    }
}
