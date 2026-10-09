//! HTTP extraction and failure framing. Connector policy stays in feature owners.
use crate::OperationError;
use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};
use spiral_robotics_common::command_failure::{CommandFailure, CommandFailureCode};

#[derive(Debug)]
pub(crate) struct HttpFailure {
    status: StatusCode,
    failure: CommandFailure,
}
impl HttpFailure {
    fn rejection(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            failure: CommandFailure::new(CommandFailureCode::InvalidInput, message),
        }
    }
    pub(super) async fn not_found() -> Self {
        CommandFailure::new(
            CommandFailureCode::NotFound,
            "Unknown robot connector endpoint",
        )
        .into()
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

/// A 201 JSON response.
pub(crate) struct Created<T>(pub(crate) T);
impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}
