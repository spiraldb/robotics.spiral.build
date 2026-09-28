//! Static browser delivery and client-side navigation, with no UI rendering.

use std::{convert::Infallible, path::PathBuf};

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

pub(super) struct StudioWebAssets {
    public: PathBuf,
}

impl StudioWebAssets {
    pub(super) fn new(public: PathBuf) -> Self {
        Self { public }
    }

    pub(super) fn router(self, api: Router) -> Router {
        let index = self.public.join("index.html");
        let files = ServeDir::new(self.public).fallback(get(Self::navigation).with_state(index));
        Router::new().nest("/api", api).fallback_service(files)
    }

    /// Only HTML navigation falls back to the application entrypoint. Missing
    /// scripts, styles, and API endpoints must keep their real 404 response.
    async fn navigation(State(index): State<PathBuf>, request: Request) -> Response {
        let html_navigation = request
            .headers()
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|accept| {
                accept.split(',').any(|part| {
                    part.split(';')
                        .next()
                        .is_some_and(|mime| mime.trim() == "text/html")
                })
            });
        if !html_navigation {
            return StatusCode::NOT_FOUND.into_response();
        }
        let result: Result<_, Infallible> = ServeFile::new(index).oneshot(request).await;
        match result {
            Ok(response) => response.into_response(),
            Err(never) => match never {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Method};

    struct WebFixture {
        _public: tempfile::TempDir,
        router: Router,
    }

    impl WebFixture {
        fn new() -> Self {
            let public = tempfile::tempdir().unwrap();
            std::fs::create_dir(public.path().join("assets")).unwrap();
            std::fs::write(
                public.path().join("index.html"),
                "<!doctype html><div id=\"main\"></div>",
            )
            .unwrap();
            std::fs::write(public.path().join("assets/app.wasm"), b"\0asm\x01\0\0\0").unwrap();
            let api = Router::new()
                .route("/status", get(|| async { "ready" }))
                .fallback(|| async { StatusCode::NOT_FOUND });
            Self {
                router: StudioWebAssets::new(public.path().to_owned()).router(api),
                _public: public,
            }
        }

        async fn request(&self, path: &str, accept: &str) -> Response {
            self.router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::ACCEPT, accept)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn browser_routes_receive_the_static_entrypoint_without_ssr() {
        let fixture = WebFixture::new();
        for path in [
            "/",
            "/live",
            "/data",
            "/curation",
            "/training",
            "/rollout",
            "/settings",
        ] {
            let response = fixture.request(path, "text/html").await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(&body[..], b"<!doctype html><div id=\"main\"></div>");
        }
    }

    #[tokio::test]
    async fn wasm_assets_keep_their_mime_type_and_head_semantics() {
        let fixture = WebFixture::new();
        let response = fixture.request("/assets/app.wasm", "*/*").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/wasm");
        let response = fixture
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/assets/app.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "8");
        assert!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn missing_assets_and_api_routes_never_receive_the_html_shell() {
        let fixture = WebFixture::new();
        for (path, accept) in [
            ("/assets/missing.js", "*/*"),
            ("/missing", "application/json"),
            ("/api/missing", "text/html"),
            ("/api", "text/html"),
        ] {
            assert_eq!(
                fixture.request(path, accept).await.status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        let response = fixture.request("/api/status", "application/json").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            &axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()[..],
            b"ready"
        );
    }
}
