//! Exercise the actual router with ordinary HTTP requests.
use super::StudioApi;
use crate::projects::context::StudioContext;
use crate::projects::tests::{ENDPOINT, ProjectFixture};
use crate::robotics::cameras::Frame;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::{get, post},
};
use bytes::Bytes;
use serde_json::Value;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_storage::bundle::ProjectBundle;
use std::sync::Arc;
use tokio::sync::Notify;
use tower::ServiceExt;

#[test]
fn multipart_parts_carry_an_accurate_length() {
    let frame = Frame::new(
        Bytes::from_static(&[0xff, 0xd8, 0x00, 0xff, 0xd9]),
        1,
        std::time::Instant::now(),
    );

    let part = frame.multipart_part();
    let text = String::from_utf8_lossy(&part);

    assert!(text.starts_with("--spiral-robotics-frame\r\nContent-Type: image/jpeg\r\n"));
    assert!(text.contains("Content-Length: 5\r\n\r\n"));
    assert!(part.ends_with(b"\r\n"));
    assert_eq!(part.len(), 5 + part.len() - 5);
}

struct HttpFixture {
    project: ProjectFixture,
    router: Router,
}
impl HttpFixture {
    fn new() -> Self {
        let project = ProjectFixture::new();
        let router = StudioApi::router(StudioContext::new(
            project.projects.clone(),
            crate::account::SpiralAccount::offline(),
        ));
        Self { project, router }
    }
    /// Name the fixture's project the way the browser does.
    fn scoped(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request
            .header(ProjectKey::ENDPOINT_HEADER, ENDPOINT)
            .header(ProjectKey::PROJECT_HEADER, "http_project")
    }
    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert!(
            response
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| panic!("Invalid JSON: {}", String::from_utf8_lossy(&bytes)));
        (status, json)
    }
    async fn shutdown(self) {
        self.project.projects.hardware().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn scoped_routes_reject_a_missing_or_stale_project_before_reading_the_body() {
    let fixture = HttpFixture::new();
    for (method, path) in [("GET", "/robots"), ("POST", "/recording")] {
        for endpoint in [None, Some("https://stale.spiraldb.dev")] {
            let mut request = Request::builder().method(method).uri(path);
            if let Some(endpoint) = endpoint {
                request = request
                    .header(ProjectKey::ENDPOINT_HEADER, endpoint)
                    .header(ProjectKey::PROJECT_HEADER, "http_project");
            }
            let (status, failure) = fixture.send(request.body(Body::empty()).unwrap()).await;
            let (expected_status, expected_code) = match endpoint {
                Some(_) => (StatusCode::CONFLICT, "stale_project"),
                None => (StatusCode::BAD_REQUEST, "invalid_input"),
            };
            assert_eq!(status, expected_status, "{method} {path}: {failure}");
            assert_eq!(failure["code"], expected_code);
        }
    }
    fixture.shutdown().await;
}

#[tokio::test]
async fn snapshots_are_plain_gets_of_the_project_a_request_names() {
    let fixture = HttpFixture::new();
    let (status, saved) = fixture
        .send(
            Request::builder()
                .uri("/settings/connection")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["endpoint"], ENDPOINT);
    for path in [
        "/robots/telemetry",
        "/recording/signals?generation=0&offset=0",
        "/training/runs",
        "/rollout",
    ] {
        let (status, value) = fixture
            .send(
                HttpFixture::scoped(Request::builder().uri(path))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{path}: {value}");
        assert!(value.is_object() || value.is_array());
    }
    assert_eq!(fixture.project.projects.opened().len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn malformed_requests_and_unknown_routes_have_structured_failures() {
    let fixture = HttpFixture::new();
    for (method, path, body, content_type, expected) in [
        (
            "POST",
            "/recording",
            "{",
            Some("application/json"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/recording",
            "{}",
            Some("application/json"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "POST",
            "/recording",
            r#"{"dataset_id":"example","typo":true}"#,
            Some("application/json"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "POST",
            "/recording",
            "{}",
            None,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "GET",
            "/recording/signals?generation=bad&offset=0",
            "",
            None,
            StatusCode::BAD_REQUEST,
        ),
        ("GET", "/unknown", "", None, StatusCode::NOT_FOUND),
        (
            "POST",
            "/robots/telemetry",
            "",
            None,
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let mut request = HttpFixture::scoped(Request::builder().method(method).uri(path));
        if let Some(content_type) = content_type {
            request = request.header("Content-Type", content_type);
        }
        let (status, failure) = fixture.send(request.body(Body::from(body)).unwrap()).await;
        assert_eq!(status, expected, "{method} {path}: {failure}");
        assert!(failure["code"].is_string());
        assert!(failure["message"].is_string());
    }
    // Every chunk shares one megabyte, so streaming past the bound stays cheap.
    let chunk = Bytes::from(vec![0; 1 << 20]);
    let chunks = ProjectBundle::MAX_BYTES / chunk.len() + 1;
    let upload = futures_util::stream::iter(
        (0..chunks).map(move |_| Ok::<_, std::io::Error>(chunk.clone())),
    );
    let oversized = HttpFixture::scoped(Request::builder().method("POST").uri("/project/import"))
        .body(Body::from_stream(upload))
        .unwrap();
    assert_eq!(
        fixture.send(oversized).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn rejected_scope_does_not_consume_an_upload() {
    let fixture = HttpFixture::new();
    let stream = futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<bytes::Bytes, std::io::Error>>> {
            panic!("upload must not be polled before project scope is checked")
        },
    );
    let request = Request::builder()
        .method("POST")
        .uri("/project/import")
        .header("Content-Type", "application/vnd.spiral-robotics.project")
        .body(Body::from_stream(stream))
        .unwrap();
    assert_eq!(fixture.send(request).await.0, StatusCode::BAD_REQUEST);
    fixture.shutdown().await;
}

/// Work that parks until released, and reports when it starts, finishes, or
/// is dropped unfinished.
#[derive(Clone, Default)]
struct ParkedWork {
    started: Arc<Notify>,
    release: Arc<Notify>,
    finished: Arc<Notify>,
    dropped: Arc<Notify>,
}
impl ParkedWork {
    async fn run(self) -> &'static str {
        let mut unfinished = DropNotice(Some(self.dropped.clone()));
        self.started.notify_one();
        self.release.notified().await;
        unfinished.0 = None;
        self.finished.notify_one();
        "done"
    }
}
struct DropNotice(Option<Arc<Notify>>);
impl Drop for DropNotice {
    fn drop(&mut self) {
        if let Some(notice) = self.0.take() {
            notice.notify_one();
        }
    }
}

#[tokio::test]
async fn a_dropped_caller_cancels_a_read_but_not_a_command() {
    let (read, command) = (ParkedWork::default(), ParkedWork::default());
    let reads = Router::new().route(
        "/read",
        get({
            let read = read.clone();
            move || read.clone().run()
        }),
    );
    let commands = Router::new()
        .route(
            "/command",
            post({
                let command = command.clone();
                move || command.clone().run()
            }),
        )
        .layer(axum::middleware::from_fn(StudioApi::run_to_completion));
    let router = reads.merge(commands);
    let within = |notice: Arc<Notify>| async move {
        tokio::time::timeout(std::time::Duration::from_secs(1), notice.notified())
            .await
            .expect("the parked work should report within a second")
    };
    for (work, method, path) in [(&read, "GET", "/read"), (&command, "POST", "/command")] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty());
        let caller = tokio::spawn(router.clone().oneshot(request.unwrap()));
        within(work.started.clone()).await;
        caller.abort();
        let _ = caller.await;
    }
    within(read.dropped.clone()).await;
    command.release.notify_one();
    within(command.finished.clone()).await;
}
