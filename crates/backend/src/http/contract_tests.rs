//! Exercise the actual router with ordinary HTTP requests, without Dioxus bindings.
use super::routes::StudioApi;
use crate::projects::context::StudioContext;
use crate::projects::tests::{ENDPOINT, ProjectFixture};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use spiral_robotics_common::project::ProjectKey;
use tower::ServiceExt;

struct HttpFixture {
    project: ProjectFixture,
    router: Router,
}
impl HttpFixture {
    fn new() -> Self {
        let project = ProjectFixture::new();
        let router = StudioApi::router(StudioContext::new(project.projects.clone()));
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
async fn every_documented_scoped_endpoint_is_registered_and_rejects_a_missing_or_stale_project() {
    let fixture = HttpFixture::new();
    let contract = StudioApi::routes().into_document();
    let mut count = 0;
    for (path, methods) in contract["paths"].as_object().unwrap() {
        for (method, operation) in methods.as_object().unwrap() {
            if !operation["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == ProjectKey::ENDPOINT_HEADER)
            {
                continue;
            }
            count += 1;
            let path = path
                .replace("{id}", "test-id")
                .replace("{dataset_id}", "test-dataset");
            for endpoint in [None, Some("https://stale.spiraldb.dev")] {
                let mut request = Request::builder()
                    .method(method.to_uppercase().as_str())
                    .uri(&path);
                if let Some(endpoint) = endpoint {
                    request = request
                        .header(ProjectKey::ENDPOINT_HEADER, endpoint)
                        .header(ProjectKey::PROJECT_HEADER, "http_project");
                }
                let (status, failure) = fixture.send(request.body(Body::empty()).unwrap()).await;
                let expected = if endpoint.is_some() {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::BAD_REQUEST
                };
                assert_eq!(status, expected, "{method} {path}: {failure}");
                assert_eq!(
                    failure["code"],
                    if endpoint.is_some() {
                        "stale_project"
                    } else {
                        "invalid_input"
                    }
                );
            }
        }
    }
    assert_eq!(count, 73);
    for retired in [
        "/robots/setup",
        "/serial-ports/identify",
        "/robots/import",
        "/training",
        "/rollout",
        "/rollout/stop",
        "/models/status",
        "/models/complete",
        "/models/decide",
        "/datasets/robot-discovery/read-description",
    ] {
        assert!(contract["paths"].get(retired).is_none(), "{retired}");
    }
    assert!(contract["paths"]["/robots"].get("patch").is_none());
    assert!(contract["paths"]["/training/runs/{id}"]["get"].is_object());
    assert!(contract["paths"]["/rollout-runs/{id}/stop"]["post"].is_object());
    assert!(contract["paths"].get("/workflow/commands").is_none());
    assert!(contract["paths"].get("/workflow/assistant").is_none());
    assert!(contract["paths"]["/hardware/status"]["get"].is_object());
    assert!(contract["paths"]["/curations/save"]["post"].is_object());
    assert!(contract["paths"]["/curation-runs"]["post"].is_object());
    assert!(contract["paths"].get("/curation-runs/{id}").is_none());
    assert!(contract["paths"]["/explorations"]["get"].is_object());
    assert!(contract["paths"]["/explorations/{id}"]["get"].is_object());
    assert_eq!(
        contract["paths"]["/explorations/save"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ExplorationDocument"
    );
    assert!(contract["paths"]["/queries/episode"]["post"].is_object());
    assert!(contract["paths"]["/datasets/robot"]["post"].is_object());
    assert!(contract["paths"].get("/project/setup").is_none());
    assert!(contract["paths"].get("/project/status").is_none());
    for path in ["/credentials", "/proxy", "/settings/connection"] {
        for (method, operation) in contract["paths"][path].as_object().unwrap() {
            if path == "/settings/connection" && method == "put" {
                continue;
            }
            assert!(
                operation["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|parameter| parameter["name"] != ProjectKey::ENDPOINT_HEADER)
            );
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
        "/rollout-runs",
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

#[test]
fn dataset_catalogs_and_episode_playback_keep_distinct_wire_contracts() {
    let contract = StudioApi::routes().into_document();
    let paths = &contract["paths"];
    assert_eq!(
        paths["/datasets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["items"]
            ["$ref"],
        "#/components/schemas/DatasetSummary"
    );
    let topics = &paths["/datasets/episode-topics"]["post"];
    assert_eq!(
        topics["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/EpisodeSelection"
    );
    assert_eq!(
        topics["responses"]["200"]["content"]["application/json"]["schema"]["items"]["$ref"],
        "#/components/schemas/DatasetTopic"
    );
    let messages = &paths["/datasets/episode-messages"]["post"];
    assert_eq!(
        messages["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/MessageRead"
    );
    assert_eq!(
        messages["responses"]["200"]["content"]["application/vnd.apache.arrow.stream"]["schema"]["format"],
        "binary"
    );
    // Nanosecond timestamps must survive the browser's numeric precision limit.
    assert_eq!(
        contract["components"]["schemas"]["VideoPacket"]["properties"]["timestamp_ns"]["type"],
        "string"
    );
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
        (
            "POST",
            "/project/import",
            "",
            None,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
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
    let oversized = HttpFixture::scoped(Request::builder().method("POST").uri("/project/import"))
        .header("Content-Type", "application/vnd.spiral-robotics.project")
        .header(
            "Content-Length",
            (crate::data::commands::MAX_PROJECT_IMPORT_BYTES + 1).to_string(),
        )
        .body(Body::empty())
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

#[tokio::test]
async fn openapi_is_available_without_a_ready_project_and_all_references_resolve() {
    let fixture = HttpFixture::new();
    let (status, contract) = fixture
        .send(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(contract["openapi"], "3.1.0");
    let mut pending = vec![&contract];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    assert!(
                        contract
                            .pointer(reference.strip_prefix('#').unwrap())
                            .is_some(),
                        "Unresolved {reference}"
                    );
                }
                pending.extend(object.values());
            }
            Value::Array(values) => pending.extend(values),
            _ => (),
        }
    }
    assert_eq!(
        contract["components"]["schemas"]["RecordingStart"]["required"],
        json!(["dataset_id", "leader", "follower"])
    );
    assert_eq!(
        contract["paths"]["/robots/telemetry"]["get"]["requestBody"],
        Value::Null
    );
    if let Ok(path) = std::env::var("SPIRAL_HTTP_SCHEMA_OUTPUT") {
        std::fs::write(path, serde_json::to_string_pretty(&contract).unwrap()).unwrap();
    }
    fixture.shutdown().await;
}
