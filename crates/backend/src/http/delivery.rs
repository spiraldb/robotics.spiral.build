//! HTTP extraction and failure framing. Application policy stays in feature owners.
use crate::OperationError;
use crate::account::SpiralSession;
use crate::projects::context::{ProjectContext, StudioContext};
use axum::{
    Json,
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Path, Request},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};
use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};
use spiral_robotics_common::project::ProjectKey;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct HttpFailure {
    status: StatusCode,
    failure: CommandFailure,
}
impl HttpFailure {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::from(CommandFailure::new(
            CommandFailureCode::InvalidInput,
            message,
        ))
    }
    pub(crate) fn rejection(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            failure: CommandFailure::new(CommandFailureCode::InvalidInput, message),
        }
    }
    pub(super) async fn not_found() -> Self {
        CommandFailure::new(CommandFailureCode::NotFound, "Unknown API endpoint").into()
    }
    pub(super) async fn method_not_allowed() -> Self {
        Self::rejection(
            StatusCode::METHOD_NOT_ALLOWED,
            "HTTP method is not supported for this endpoint",
        )
    }
}
impl From<CommandFailure> for HttpFailure {
    fn from(failure: CommandFailure) -> Self {
        Self {
            status: StatusCode::from_u16(failure.code().http_status())
                .expect("declared failure status"),
            failure,
        }
    }
}
impl From<OperationError> for HttpFailure {
    fn from(error: OperationError) -> Self {
        CommandFailure::from(error).into()
    }
}
impl IntoResponse for HttpFailure {
    fn into_response(self) -> Response {
        (self.status, Json(self.failure)).into_response()
    }
}
impl IntoResponse for OperationError {
    fn into_response(self) -> Response {
        HttpFailure::from(self).into_response()
    }
}

/// The browser session a request carries, which the API's session gate found.
pub(crate) struct SignedIn(pub(crate) Arc<SpiralSession>);
impl<S: Send + Sync> FromRequestParts<S> for SignedIn {
    type Rejection = HttpFailure;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Arc<SpiralSession>>()
            .cloned()
            .map(Self)
            .ok_or_else(|| {
                OperationError::Unauthenticated("Sign in to Spiral to continue.".to_owned()).into()
            })
    }
}

/// Open the project a request names, as its session's user, before reading
/// its body. The endpoint header is required; a missing or empty project
/// header names the endpoint before a project is chosen.
pub(crate) struct Project(pub(crate) ProjectContext);
impl FromRequestParts<StudioContext> for Project {
    type Rejection = HttpFailure;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &StudioContext,
    ) -> Result<Self, Self::Rejection> {
        let SignedIn(session) = SignedIn::from_request_parts(parts, state).await?;
        let endpoint = Self::header(parts, ProjectKey::ENDPOINT_HEADER)?
            .filter(|endpoint| !endpoint.is_empty())
            .ok_or_else(|| {
                HttpFailure::invalid(format!("{} is required", ProjectKey::ENDPOINT_HEADER))
            })?;
        let project = Self::header(parts, ProjectKey::PROJECT_HEADER)?.unwrap_or_default();
        let key = ProjectKey::new(endpoint, project);
        Ok(Self(state.open(&session, &key).await?))
    }
}
impl Project {
    fn header<'a>(parts: &'a Parts, name: &str) -> Result<Option<&'a str>, HttpFailure> {
        let mut values = parts.headers.get_all(name).iter();
        let value = values
            .next()
            .map(|value| {
                value
                    .to_str()
                    .map_err(|_| HttpFailure::invalid(format!("{name} must be visible ASCII")))
            })
            .transpose()?;
        if values.next().is_some() {
            return Err(HttpFailure::invalid(format!(
                "Supply at most one {name} header"
            )));
        }
        Ok(value)
    }
}
pub(crate) struct ApiJson<T>(pub(crate) T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequest<S> for ApiJson<T> {
    type Rejection = HttpFailure;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error| HttpFailure::rejection(error.status(), error.body_text()))
    }
}
/// A raw body, bounded by the route's `DefaultBodyLimit`.
pub(crate) struct ApiBytes(pub(crate) Bytes);
impl<S: Send + Sync> FromRequest<S> for ApiBytes {
    type Rejection = HttpFailure;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Bytes::from_request(request, state)
            .await
            .map(Self)
            .map_err(|error| HttpFailure::rejection(error.status(), error.body_text()))
    }
}
pub(crate) struct ApiPath<T>(pub(crate) T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for ApiPath<T> {
    type Rejection = HttpFailure;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|error| HttpFailure::rejection(error.status(), error.body_text()))
    }
}

/// A 201 JSON response.
pub(crate) struct Created<T>(pub(crate) T);
impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}

/// A bounded IPC stream with its native schema and record batches.
pub(crate) struct ArrowResponse(pub(crate) Vec<u8>);
impl IntoResponse for ArrowResponse {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_TYPE, "application/vnd.apache.arrow.stream")],
            self.0,
        )
            .into_response()
    }
}
