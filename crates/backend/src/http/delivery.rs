//! HTTP extraction and failure framing. Application policy stays in command owners.
use crate::project_session::context::ProjectContext;
use crate::{BackendCommandError, OperationError};
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};

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
    pub(crate) async fn not_found() -> Self {
        CommandFailure::new(CommandFailureCode::NotFound, "Unknown API endpoint").into()
    }
    pub(crate) async fn method_not_allowed() -> Self {
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

/// Resolve identity before reading a body. Command owners still acquire admission
/// when executing, closing the race with a subsequent project replacement.
pub(crate) struct ScopedProject<Capability>(pub(crate) Capability);
impl<Capability: From<ProjectContext>> FromRequestParts<ProjectContext>
    for ScopedProject<Capability>
{
    type Rejection = HttpFailure;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &ProjectContext,
    ) -> Result<Self, Self::Rejection> {
        let mut sessions = parts.headers.get_all("X-Project-Session").iter();
        let session = sessions
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| HttpFailure::invalid("X-Project-Session is required"))?;
        if sessions.next().is_some() {
            return Err(HttpFailure::invalid(
                "Supply exactly one X-Project-Session header",
            ));
        }
        Ok(Self(Capability::from(state.for_session(session)?)))
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
