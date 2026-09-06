//! Browser-native camera preview delivery.

use super::*;
use std::convert::Infallible;

impl AppState {
    /// Camera previews stay HTTP because browser image elements consume MJPEG URLs.
    pub(super) fn device_bootstrap_router() -> Router<Self> {
        Router::new().route("/cameras/stream.mjpeg", get(Self::camera_stream))
    }
}

/// A camera is addressed by query string because its identifier may be a device
/// path containing slashes.
#[derive(Debug, Deserialize)]
struct CameraQuery {
    id: String,
}

impl AppState {
    async fn camera_stream(
        State(state): State<Self>,
        Query(query): Query<CameraQuery>,
    ) -> Result<Response, (StatusCode, String)> {
        let receiver = state
            .cameras()
            .subscribe(&query.id)
            .map_err(|error| (StatusCode::NOT_FOUND, error))?;

        let frames = tokio_stream::wrappers::WatchStream::new(receiver)
            .filter_map(|frame| async move { frame })
            .scan(0_u64, |last, frame| {
                let fresh = frame.sequence() != *last;
                *last = frame.sequence();
                futures_util::future::ready(Some(fresh.then_some(frame)))
            })
            .filter_map(|frame| async move { frame })
            .map(|frame| Ok::<Bytes, Infallible>(frame.multipart_part()));

        Ok(Response::builder()
            .header(
                header::CONTENT_TYPE,
                format!("multipart/x-mixed-replace; boundary={MJPEG_BOUNDARY}"),
            )
            .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
            .header(header::PRAGMA, "no-cache")
            .header(header::EXPIRES, "0")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Body::from_stream(frames))
            .expect("a valid MJPEG response"))
    }
}

impl Frame {
    /// Wrap this JPEG as a part of a `multipart/x-mixed-replace` response.
    pub(super) fn multipart_part(&self) -> Bytes {
        let mut part = BytesMut::with_capacity(self.bytes().len() + 96);
        part.put_slice(b"--");
        part.put_slice(MJPEG_BOUNDARY.as_bytes());
        part.put_slice(b"\r\nContent-Type: image/jpeg\r\nContent-Length: ");
        part.put_slice(self.bytes().len().to_string().as_bytes());
        part.put_slice(b"\r\n\r\n");
        part.put_slice(self.bytes());
        part.put_slice(b"\r\n");
        part.freeze()
    }
}
