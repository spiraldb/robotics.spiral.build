//! Exercise the actual router with ordinary HTTP requests.
use super::StudioApi;
use crate::projects::tests::{ENDPOINT, ProjectFixture};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use bytes::Bytes;
use serde_json::Value;
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_storage::bundle::ProjectBundle;
use std::sync::Arc;
use tokio::sync::Notify;
use tower::ServiceExt;

struct HttpFixture {
    project: ProjectFixture,
    router: Router,
}
impl HttpFixture {
    async fn new() -> Self {
        let project = ProjectFixture::new().await;
        project.workspace("http_project").await;
        let router = StudioApi::router(project.context());
        Self { project, router }
    }
    /// Name the fixture's project the way the browser does.
    fn scoped(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request
            .header(ProjectKey::ENDPOINT_HEADER, ENDPOINT)
            .header(ProjectKey::PROJECT_HEADER, "http_project")
    }
    /// Send `request` from the fixture's signed-in browser.
    async fn send(&self, mut request: Request<Body>) -> (StatusCode, Value) {
        request
            .headers_mut()
            .insert(header::COOKIE, self.project.cookie().parse().unwrap());
        self.send_anonymously(request).await
    }
    async fn send_anonymously(&self, request: Request<Body>) -> (StatusCode, Value) {
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
}

#[tokio::test]
async fn every_route_requires_a_live_session() {
    let fixture = HttpFixture::new().await;
    for cookie in [None, Some("studio_oauth_session=forged")] {
        for (method, path) in [
            ("GET", "/settings/connection"),
            ("GET", "/providers"),
            ("PUT", "/settings"),
            (
                "GET",
                "/proxy?url=https%3A%2F%2Fopenrouter.ai%2Fapi%2Fv1%2Fmodels",
            ),
            ("GET", "/no/such/endpoint"),
        ] {
            let mut request = HttpFixture::scoped(Request::builder().method(method).uri(path));
            if let Some(cookie) = cookie {
                request = request.header(header::COOKIE, cookie);
            }
            let (status, failure) = fixture
                .send_anonymously(request.body(Body::empty()).unwrap())
                .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert_eq!(failure["code"], "unauthenticated");
        }
    }
}

#[tokio::test]
async fn scoped_routes_reject_a_missing_or_stale_project_before_reading_the_body() {
    let fixture = HttpFixture::new().await;
    for (method, path) in [
        ("GET", "/training/runs"),
        ("POST", "/datasets/demo/recordings/take-1"),
    ] {
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
}

#[tokio::test]
async fn snapshots_are_plain_gets_of_the_project_a_request_names() {
    let fixture = HttpFixture::new().await;
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
    for path in ["/settings", "/training/runs"] {
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
}

#[tokio::test]
async fn a_browser_creates_chooses_and_deletes_workspaces() {
    let fixture = HttpFixture::new().await;
    let request = |method: &str, uri: &str, body: Option<Value>| {
        let request = Request::builder().method(method).uri(uri);
        match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap()
    };
    let working_in = async || {
        let (_, saved) = fixture
            .send(request("GET", "/settings/connection", None))
            .await;
        saved["project_id"].clone()
    };
    let (status, created) = fixture
        .send(request(
            "POST",
            "/workspaces",
            Some(serde_json::json!({"name": "Lab arms", "project_id": "project_b"})),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["project_id"], "project_b");
    let existing = serde_json::json!({"name": "Bench", "project_id": "project_a"});
    for expected in [StatusCode::OK, StatusCode::CONFLICT] {
        let (status, _) = fixture
            .send(request("POST", "/workspaces", Some(existing.clone())))
            .await;
        assert_eq!(status, expected);
    }
    let (_, listed) = fixture.send(request("GET", "/workspaces", None)).await;
    let names: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Bench", "Lab arms"]);

    let choice = serde_json::json!({"project_id": "project_b"});
    let (status, chosen) = fixture
        .send(request("PUT", "/settings/connection", Some(choice)))
        .await;
    assert_eq!(status, StatusCode::OK, "{chosen}");
    assert_eq!(working_in().await, "project_b");
    let (status, _) = fixture
        .send(request("DELETE", "/workspaces/project_b", None))
        .await;
    assert_eq!(status, StatusCode::OK);
    // The browser goes on in the oldest workspace it can open.
    assert_eq!(working_in().await, "project_a");
}

#[tokio::test]
async fn the_browser_learns_of_no_model_provider_without_its_key() {
    let fixture = HttpFixture::new().await;
    let request = Request::builder()
        .uri("/providers")
        .body(Body::empty())
        .unwrap();
    let (status, providers) = fixture.send(request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(providers["llm"].is_null());
}

#[tokio::test]
async fn malformed_requests_and_unknown_routes_have_structured_failures() {
    let fixture = HttpFixture::new().await;
    for (method, path, body, content_type, expected) in [
        (
            "PUT",
            "/settings",
            "{",
            Some("application/json"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PUT",
            "/settings",
            "{}",
            Some("application/json"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "PUT",
            "/settings",
            r#"{"name":"example","typo":true}"#,
            Some("application/json"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "PUT",
            "/settings",
            "{}",
            None,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        ("GET", "/unknown", "", None, StatusCode::NOT_FOUND),
        (
            "PUT",
            "/training/runs",
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
}

#[tokio::test]
async fn rejected_scope_does_not_consume_an_upload() {
    let fixture = HttpFixture::new().await;
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
