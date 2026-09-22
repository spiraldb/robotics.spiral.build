//! HTTP extraction and failure framing. Application policy stays in command owners.
use crate::projects::context::{ProjectContext, StudioContext};
use crate::{BackendCommandError, OperationError};
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};
use spiral_robotics_common::project::ProjectKey;

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
impl From<BackendCommandError> for HttpFailure {
    fn from(error: BackendCommandError) -> Self {
        CommandFailure::from(error).into()
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

/// Open the project a request names before reading its body. The endpoint
/// header is required; a missing or empty project header names the endpoint
/// before a project is chosen.
pub(crate) struct ScopedProject<Capability>(pub(crate) Capability);
impl<Capability: From<ProjectContext>> FromRequestParts<StudioContext>
    for ScopedProject<Capability>
{
    type Rejection = HttpFailure;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &StudioContext,
    ) -> Result<Self, Self::Rejection> {
        let endpoint = Self::header(parts, ProjectKey::ENDPOINT_HEADER)?
            .filter(|endpoint| !endpoint.is_empty())
            .ok_or_else(|| {
                HttpFailure::invalid(format!("{} is required", ProjectKey::ENDPOINT_HEADER))
            })?;
        let project = Self::header(parts, ProjectKey::PROJECT_HEADER)?.unwrap_or_default();
        Ok(Self(Capability::from(
            state.open(&ProjectKey::new(endpoint, project))?,
        )))
    }
}
impl<Capability> ScopedProject<Capability> {
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
pub(crate) struct ApiQuery<T>(pub(crate) T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = HttpFailure;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
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
