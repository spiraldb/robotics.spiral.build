//! HTTP endpoints for robot connectors: where a connector links, and where
//! browsers reach their workspace's connector through Studio.

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, FromRequest, FromRequestParts, OriginalUri, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::robot_connector::{
    HEALTH_HEADER, RELAY_PREFIX, REQUEST_HEADER, RobotConnectorHealth, RobotConnectorToken,
    STATUS_HEADER, WATCH_HEADER,
};

use super::RobotConnectors;
use super::episodes::EpisodeEndpoints;
use super::link::{ConnectorLink, RelayedAnswer, RelayedRequest};
use super::live::CameraViewer;
use crate::OperationError;
use crate::http::delivery::{ApiBytes, ApiPath, ApiQuery, HttpFailure, Project, SignedIn};
use crate::projects::context::StudioContext;

/// Relayed requests and their answers carry an episode's camera preview whole.
const RELAYED_BODY_LIMIT: usize = 512 * 1024 * 1024;

pub(crate) struct RobotConnectorEndpoints;

impl RobotConnectorEndpoints {
    /// Where a connector links. It presents its workspace's token, not a
    /// browser's session.
    pub(crate) fn link() -> Router<StudioContext> {
        Router::new()
            .route("/robot-connector/link/next", get(Self::next_request))
            .route(
                "/robot-connector/link/answers/{id}",
                post(Self::answer).layer(DefaultBodyLimit::max(RELAYED_BODY_LIMIT)),
            )
            .route(
                "/robot-connector/link/telemetry",
                post(Self::publish_telemetry),
            )
            .route("/robot-connector/link/cameras", post(Self::publish_frame))
            .merge(EpisodeEndpoints::link())
    }

    /// The workspace's connector, as its browsers reach it: its health, live
    /// data, and uploaded episodes from Studio, and every other request relayed
    /// to it.
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .merge(EpisodeEndpoints::reads())
            .route("/robot-connector/health", get(Self::health))
            .route("/robot-connector/robots/telemetry", get(Self::telemetry))
            .route(
                "/robot-connector/cameras/stream.mjpeg",
                get(Self::camera_stream),
            )
            .route(
                "/robot-connector/{*path}",
                any(Self::relay).layer(DefaultBodyLimit::max(RELAYED_BODY_LIMIT)),
            )
    }

    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route("/settings/robot-connector/token", post(Self::issue_token))
            .merge(EpisodeEndpoints::commands())
    }

    /// The connector's long poll, which reports its health. Every answer
    /// names the streams watched now.
    async fn next_request(
        LinkedConnector(link): LinkedConnector,
        headers: HeaderMap,
    ) -> Result<Response, HttpFailure> {
        let health: RobotConnectorHealth = headers
            .get(HEALTH_HEADER)
            .ok_or_else(|| format!("{HEALTH_HEADER} is required"))
            .and_then(|health| {
                serde_json::from_slice(health.as_bytes())
                    .map_err(|error| format!("{HEALTH_HEADER} is malformed: {error}"))
            })
            .map_err(HttpFailure::invalid)?;
        let request = link.next_request(health).await?;
        let watched = Self::json_header(&link.live().announce())?;
        let Some(request) = request else {
            return Ok((StatusCode::NO_CONTENT, [(WATCH_HEADER, watched)]).into_response());
        };
        let (head, content_type, body) = request.into_parts();
        let mut response = (
            [
                (WATCH_HEADER, watched),
                (REQUEST_HEADER, Self::json_header(&head)?),
            ],
            Body::from(body),
        )
            .into_response();
        if let Some(content_type) = content_type {
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type);
        }
        Ok(response)
    }

    async fn answer(
        LinkedConnector(link): LinkedConnector,
        ApiPath(id): ApiPath<u64>,
        answer: RelayedAnswer,
    ) -> Result<StatusCode, OperationError> {
        if link.answer(id, answer) {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(OperationError::NotFound(format!(
                "No relayed request {id} awaits an answer"
            )))
        }
    }

    async fn publish_telemetry(
        LinkedConnector(link): LinkedConnector,
        ApiBytes(sample): ApiBytes,
    ) -> StatusCode {
        link.live().publish_telemetry(sample);
        StatusCode::NO_CONTENT
    }

    async fn publish_frame(
        LinkedConnector(link): LinkedConnector,
        ApiQuery(camera): ApiQuery<CameraFrameQuery>,
        ApiBytes(frame): ApiBytes,
    ) -> StatusCode {
        link.live().publish_frame(&camera.id, frame);
        StatusCode::NO_CONTENT
    }

    async fn health(
        Project(project): Project,
    ) -> Result<Json<RobotConnectorHealth>, OperationError> {
        let link = project.studio().robot_connectors().linked(&project)?;
        link.health()
            .map(Json)
            .ok_or_else(RobotConnectors::unlinked)
    }

    /// The telemetry the connector posted last, as its own
    /// `GET /api/robots/telemetry` answers it.
    async fn telemetry(Project(project): Project) -> Result<Response, OperationError> {
        let link = project.studio().robot_connectors().linked(&project)?;
        let sample = link.live().telemetry().await?;
        Ok(([(header::CONTENT_TYPE, "application/json")], sample).into_response())
    }

    /// An `<img>` element sends no headers, so the project comes in the
    /// query string; the session cookie still authorizes it.
    async fn camera_stream(
        State(state): State<StudioContext>,
        SignedIn(session): SignedIn,
        ApiQuery(query): ApiQuery<CameraStreamQuery>,
    ) -> Result<Response, OperationError> {
        let project = state.open(&session, &query.project).await?;
        let viewer = state
            .robot_connectors()
            .linked(&project)?
            .live()
            .view_camera(query.id)?;
        Ok(MjpegStream::response(viewer))
    }

    async fn relay(
        Project(project): Project,
        request: RelayedRequest,
    ) -> Result<RelayedAnswer, OperationError> {
        let link = project.studio().robot_connectors().linked(&project)?;
        link.relay(request).await
    }

    async fn issue_token(
        Project(project): Project,
    ) -> Result<Json<RobotConnectorToken>, OperationError> {
        let project_id = project.workspace_project_id()?;
        Ok(Json(
            project
                .studio()
                .robot_connectors()
                .issue_token(&project_id)
                .await?,
        ))
    }

    fn json_header(value: &impl Serialize) -> Result<HeaderValue, OperationError> {
        let json = serde_json::to_string(value).expect("a link header serializes");
        HeaderValue::from_str(&json)
            .map_err(|error| OperationError::Internal(format!("a link header is invalid: {error}")))
    }
}

/// A camera, by the ID the connector's `GET /api/hardware/status` lists. An
/// ID may be a device path with slashes, so it travels in the query string.
#[derive(Deserialize)]
struct CameraFrameQuery {
    id: String,
}

/// A camera, and the project whose connector shows it.
#[derive(Deserialize)]
struct CameraStreamQuery {
    id: String,
    #[serde(flatten)]
    project: ProjectKey,
}

/// The link of the workspace whose token a connector's request presents.
struct LinkedConnector(Arc<ConnectorLink>);

impl FromRequestParts<StudioContext> for LinkedConnector {
    type Rejection = HttpFailure;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &StudioContext,
    ) -> Result<Self, Self::Rejection> {
        let token = RobotConnectors::presented_token(parts)?;
        Ok(Self(state.robot_connectors().authenticate(token).await?))
    }
}

/// The project of the workspace whose token a connector's request presents.
pub(super) struct LinkedWorkspace(pub(super) String);

impl FromRequestParts<StudioContext> for LinkedWorkspace {
    type Rejection = HttpFailure;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &StudioContext,
    ) -> Result<Self, Self::Rejection> {
        let token = RobotConnectors::presented_token(parts)?;
        Ok(Self(state.robot_connectors().workspace(token).await?))
    }
}

/// A browser request under [`RELAY_PREFIX`], for the connector's own `/api`.
impl<S: Send + Sync> FromRequest<S> for RelayedRequest {
    type Rejection = HttpFailure;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = request.into_parts();
        let Ok(OriginalUri(uri)) = OriginalUri::from_request_parts(&mut parts, state).await;
        let path = uri.path().strip_prefix(RELAY_PREFIX).ok_or_else(|| {
            OperationError::NotFound(format!("{uri} is not a robot connector path"))
        })?;
        let path = match uri.query() {
            Some(query) => format!("/api{path}?{query}"),
            None => format!("/api{path}"),
        };
        let method = parts.method.as_str().to_owned();
        let content_type = parts.headers.get(header::CONTENT_TYPE).cloned();
        let ApiBytes(body) =
            ApiBytes::from_request(Request::from_parts(parts, body), state).await?;
        Ok(Self::new(method, path, content_type, body))
    }
}

/// A connector's answer to a relayed request: its status in
/// [`STATUS_HEADER`], its content type, and its body.
impl<S: Send + Sync> FromRequest<S> for RelayedAnswer {
    type Rejection = HttpFailure;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let status = request
            .headers()
            .get(STATUS_HEADER)
            .and_then(|status| StatusCode::from_bytes(status.as_bytes()).ok())
            .ok_or_else(|| {
                HttpFailure::invalid(format!("{STATUS_HEADER} must be the answer's HTTP status"))
            })?;
        let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
        let ApiBytes(body) = ApiBytes::from_request(request, state).await?;
        Ok(Self::new(status, content_type, body))
    }
}

/// A camera's frames as one `multipart/x-mixed-replace` response, which an
/// `<img>` element shows as video, framed as the connector framed its own.
struct MjpegStream;

impl MjpegStream {
    const BOUNDARY: &str = "spiral-robotics-frame";

    fn response(viewer: CameraViewer) -> Response {
        let parts = futures_util::stream::unfold(viewer, |mut viewer| async move {
            let frame = viewer.next_frame().await?;
            Some((Ok::<_, Infallible>(Self::part(&frame)), viewer))
        });
        Response::builder()
            .header(
                header::CONTENT_TYPE,
                format!("multipart/x-mixed-replace; boundary={}", Self::BOUNDARY),
            )
            .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
            .header(header::PRAGMA, "no-cache")
            .header(header::EXPIRES, "0")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Body::from_stream(parts))
            .expect("a valid MJPEG response")
    }

    /// One JPEG as a part of the response.
    fn part(frame: &[u8]) -> Bytes {
        let mut part = format!(
            "--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            Self::BOUNDARY,
            frame.len()
        )
        .into_bytes();
        part.extend_from_slice(frame);
        part.extend_from_slice(b"\r\n");
        Bytes::from(part)
    }
}
