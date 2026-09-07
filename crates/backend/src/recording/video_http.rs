//! Browser-native episode video delivery.

use crate::data::queries::EpisodeQueries;
use crate::http::contract::{ApiRoutes, MediaEndpoint};
use crate::project_session::context::ProjectContext;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use bytes::Bytes;
use serde::Deserialize;
use spiral_robotics_storage::video::{VideoAssetKey, VideoByteRange};

pub(crate) struct EpisodeVideo;

impl EpisodeVideo {
    /// Episode videos stay HTTP because browser video elements issue range requests.
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.media("/episodes/{id}/videos/{stream}", MediaEndpoint::new(get(Self::episode_video), serde_json::json!({"get": {
            "operationId": "episode_video", "parameters": [
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}},
                {"name": "stream", "in": "path", "required": true, "schema": {"type": "string"}},
                {"name": "session_id", "in": "query", "required": true, "schema": {"type": "string"}},
                {"name": "segment", "in": "query", "required": false, "schema": {"type": "integer", "minimum": 0, "default": 0}},
                {"name": "Range", "in": "header", "required": false, "schema": {"type": "string"}}
            ],
            "responses": {
                "200": {"description": "Complete video", "content": {"video/mp4": {"schema": {"type": "string", "format": "binary"}}}},
                "206": {"description": "Requested byte range", "content": {"video/mp4": {"schema": {"type": "string", "format": "binary"}}}}
            }
        }})["get"].take()));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteRange {
    start: u64,
    end: u64,
}

#[cfg(test)]
impl ByteRange {
    pub(crate) const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

impl ByteRange {
    pub(crate) fn parse(value: &str, length: u64) -> Option<Self> {
        if length == 0 {
            return None;
        }
        let value = value.trim().strip_prefix("bytes=")?;
        if value.contains(',') {
            return None;
        }
        let (start, end) = value.split_once('-')?;
        if start.trim().is_empty() {
            let suffix = end.trim().parse::<u64>().ok()?;
            if suffix == 0 {
                return None;
            }
            return Some(Self {
                start: length.saturating_sub(suffix.min(length)),
                end: length - 1,
            });
        }
        let start = start.trim().parse::<u64>().ok()?;
        if start >= length {
            return None;
        }
        let end = if end.trim().is_empty() {
            length - 1
        } else {
            end.trim().parse::<u64>().ok()?.min(length - 1)
        };
        (end >= start).then_some(Self { start, end })
    }

    fn unsatisfied_response(length: u64) -> Response {
        Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{length}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::empty())
            .expect("valid unsatisfied range response")
    }
}

#[derive(Debug, Deserialize)]
struct LocalVideoQuery {
    session_id: String,
    #[serde(default)]
    segment: Option<u32>,
}

impl EpisodeVideo {
    async fn episode_video(
        State(state): State<ProjectContext>,
        (Path((id, stream)), Query(query), headers): (
            Path<(String, String)>,
            Query<LocalVideoQuery>,
            HeaderMap,
        ),
    ) -> Result<Response, (StatusCode, String)> {
        let state = state
            .for_session(&query.session_id)
            .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;
        let _activity = state
            .admit()
            .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;

        if let Some(detail) = state.bootstrap().storage_unavailable() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!("SpiralDB storage is not ready: {detail}"),
            ));
        }
        // Looking up the episode first prevents an orphaned media row from becoming
        // an independent serving namespace.
        EpisodeQueries::new(&state)
            .authorize_asset(id.clone())
            .await
            .map_err(|error| (StatusCode::NOT_FOUND, error.to_string()))?;
        let asset = VideoAssetKey::new(id, stream, query.segment);
        let length = state
            .storage()
            .video_asset_size(&asset)
            .await
            .map_err(|error| (StatusCode::NOT_FOUND, error))?;
        let requested_range = match headers.get(header::RANGE) {
            None => None,
            Some(value) => match value
                .to_str()
                .ok()
                .and_then(|value| ByteRange::parse(value, length))
            {
                Some(range) => Some(range),
                None => return Ok(ByteRange::unsatisfied_response(length)),
            },
        };
        let range = requested_range.unwrap_or(ByteRange {
            start: 0,
            end: length.saturating_sub(1),
        });
        let content_length = if length == 0 {
            0
        } else {
            range.end - range.start + 1
        };
        let body = if content_length == 0 {
            Bytes::new()
        } else {
            let payload = state
                .storage()
                .video_asset_range(&asset, VideoByteRange::new(range.start, content_length))
                .await
                .map_err(|error| (StatusCode::NOT_FOUND, error))?;
            if payload.len() as u64 != content_length {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Spiral returned {} video bytes for requested range {}-{} (expected {content_length})",
                        payload.len(),
                        range.start,
                        range.end,
                    ),
                ));
            }
            Bytes::from(payload)
        };
        let mut response = Response::builder()
            .status(if requested_range.is_some() {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            })
            .header(header::CONTENT_TYPE, "video/mp4")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, content_length)
            .header(header::CACHE_CONTROL, "private, no-store");
        if requested_range.is_some() {
            response = response.header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", range.start, range.end, length),
            );
        }
        Ok(response
            .body(Body::from(body))
            .expect("valid Spiral video response"))
    }
}
