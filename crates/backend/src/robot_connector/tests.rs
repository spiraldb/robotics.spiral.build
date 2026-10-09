//! Drive Studio's real router as a robot connector and a signed-in browser
//! do, with plain HTTP requests.

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode, header, request::Builder},
    response::Response,
};
use futures_util::StreamExt as _;
use serde_json::{Value, json};
use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::robot_connector::{
    EpisodePart, HEALTH_HEADER, LINK_ANSWERS_PATH, LINK_CAMERAS_PATH, LINK_EPISODES_PATH,
    LINK_NEXT_PATH, LINK_TELEMETRY_PATH, LiveStream, RELAY_PREFIX, REQUEST_HEADER,
    RelayedRequestHead, RobotConnectorHealth, STATUS_HEADER, WATCH_HEADER,
};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tower::ServiceExt as _;

use crate::http::routes::StudioApi;
use crate::projects::tests::{ENDPOINT, ProjectFixture};

const PROJECT: &str = "lab_arms";

/// Studio's API, as served under `/api`, with one workspace and a browser
/// signed in to it.
struct LinkFixture {
    project: ProjectFixture,
    router: Router,
}

impl LinkFixture {
    async fn new() -> Self {
        let project = ProjectFixture::new().await;
        project.workspace(PROJECT).await;
        let router = Router::new().nest("/api", StudioApi::router(project.context()));
        Self { project, router }
    }

    fn health() -> RobotConnectorHealth {
        RobotConnectorHealth::new("0.1.0", true)
    }

    async fn send(&self, request: Request<Body>) -> Response {
        self.router.clone().oneshot(request).await.unwrap()
    }

    /// `request` from the browser, naming the workspace.
    fn browser(&self, request: Builder) -> Builder {
        request
            .header(header::COOKIE, self.project.cookie())
            .header(ProjectKey::ENDPOINT_HEADER, ENDPOINT)
            .header(ProjectKey::PROJECT_HEADER, PROJECT)
    }

    /// `request` from a connector presenting `token`.
    fn connector(token: &str, request: Builder) -> Builder {
        request.header(header::AUTHORIZATION, format!("Bearer {token}"))
    }

    async fn issue_token(&self) -> String {
        let request = self
            .browser(Request::post("/api/settings/robot-connector/token"))
            .body(Body::empty())
            .unwrap();
        let (status, token) = Self::json(self.send(request).await).await;
        assert_eq!(status, StatusCode::OK, "{token}");
        token["token"].as_str().unwrap().to_owned()
    }

    /// A connector's poll with `token`, waiting in the background.
    fn poll(&self, token: &str) -> JoinHandle<Response> {
        let request = Self::connector(token, Request::get(LINK_NEXT_PATH))
            .header(
                HEALTH_HEADER,
                serde_json::to_string(&Self::health()).unwrap(),
            )
            .body(Body::empty())
            .unwrap();
        self.spawn(request)
    }

    /// Send `request` in the background.
    fn spawn(&self, request: Request<Body>) -> JoinHandle<Response> {
        let router = self.router.clone();
        tokio::spawn(async move { router.oneshot(request).await.unwrap() })
    }

    /// What the workspace's linked connector reports itself as.
    async fn linked_health(&self) -> (StatusCode, Value) {
        let request = self
            .browser(Request::get(format!("{RELAY_PREFIX}/health")))
            .body(Body::empty())
            .unwrap();
        Self::json(self.send(request).await).await
    }

    /// Wait until a waiting poll links the connector.
    async fn until_linked(&self) {
        for _ in 0..200 {
            if self.linked_health().await.0 == StatusCode::OK {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the connector never linked");
    }

    /// A browser request to the connector, waiting in the background.
    fn relay(&self, request: Builder, body: Body) -> JoinHandle<Response> {
        self.spawn(self.browser(request).body(body).unwrap())
    }

    /// The streams a poll's answer names as watched.
    fn watched(response: &Response) -> Vec<LiveStream> {
        serde_json::from_slice(response.headers()[WATCH_HEADER].as_bytes()).unwrap()
    }

    async fn body(response: Response) -> Bytes {
        to_bytes(response.into_body(), usize::MAX).await.unwrap()
    }

    async fn json(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        (
            status,
            serde_json::from_slice(&Self::body(response).await).unwrap(),
        )
    }
}

#[tokio::test]
async fn a_connector_links_with_its_workspaces_current_token_only() {
    let fixture = LinkFixture::new().await;
    let settings = async || {
        let request = fixture
            .browser(Request::get("/api/settings"))
            .body(Body::empty())
            .unwrap();
        LinkFixture::json(fixture.send(request).await).await.1["robot_connector"].clone()
    };
    assert_eq!(settings().await["token_issued"], false);
    for authorization in [None, Some("Bearer rc_guessed"), Some("rc_guessed")] {
        let mut request = Request::get(LINK_NEXT_PATH);
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        let (status, failure) =
            LinkFixture::json(fixture.send(request.body(Body::empty()).unwrap()).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{authorization:?}");
        assert_eq!(failure["code"], "unauthenticated");
    }

    let token = fixture.issue_token().await;
    assert!(token.starts_with("rc_") && token.len() == 46, "{token}");
    assert_eq!(settings().await["token_issued"], true);
    let (status, failure) = fixture.linked_health().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["code"], "transport");
    assert_eq!(
        failure["message"],
        "No robot connector is linked to this workspace"
    );
    let unhealthy = LinkFixture::connector(&token, Request::get(LINK_NEXT_PATH))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        fixture.send(unhealthy).await.status(),
        StatusCode::BAD_REQUEST
    );

    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    assert_eq!(
        fixture.linked_health().await.1,
        serde_json::to_value(LinkFixture::health()).unwrap()
    );

    // Reissuing refuses the old token's waiting poll and every later one.
    let reissued = fixture.issue_token().await;
    assert_ne!(reissued, token);
    assert_eq!(poll.await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        fixture.linked_health().await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        fixture.poll(&token).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let poll = fixture.poll(&reissued);
    fixture.until_linked().await;
    poll.abort();
}

#[tokio::test]
async fn a_relayed_request_reaches_the_connector_and_its_answer_the_browser() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let (status, failure) = LinkFixture::json(
        fixture
            .relay(
                Request::get(format!("{RELAY_PREFIX}/robots")),
                Body::empty(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["code"], "transport");

    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    let browser = fixture.relay(
        Request::post(format!("{RELAY_PREFIX}/policies/load?step=3"))
            .header(header::CONTENT_TYPE, "application/octet-stream"),
        Body::from(vec![0, 1, 2, 0xff]),
    );
    let delivered = poll.await.unwrap();
    assert_eq!(delivered.status(), StatusCode::OK);
    assert!(LinkFixture::watched(&delivered).is_empty());
    let head: RelayedRequestHead =
        serde_json::from_slice(delivered.headers()[REQUEST_HEADER].as_bytes()).unwrap();
    assert_eq!(
        head,
        RelayedRequestHead::new(head.id(), "POST", "/api/policies/load?step=3")
    );
    assert_eq!(
        delivered.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        &LinkFixture::body(delivered).await[..],
        &[0, 1, 2, 0xff][..]
    );

    let answer = |id: u64| {
        LinkFixture::connector(&token, Request::post(format!("{LINK_ANSWERS_PATH}/{id}")))
            .header(STATUS_HEADER, "201")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"loaded":true}"#))
            .unwrap()
    };
    assert_eq!(
        fixture.send(answer(head.id() + 1)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture.send(answer(head.id())).await.status(),
        StatusCode::NO_CONTENT
    );
    let answered = browser.await.unwrap();
    assert_eq!(answered.status(), StatusCode::CREATED);
    assert_eq!(answered.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        &LinkFixture::body(answered).await[..],
        br#"{"loaded":true}"#
    );
    // A request is answered once.
    assert_eq!(
        fixture.send(answer(head.id())).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_request_the_connector_holds_fails_when_it_is_disconnected() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    let browser = fixture.relay(
        Request::get(format!("{RELAY_PREFIX}/robots")),
        Body::empty(),
    );
    assert_eq!(poll.await.unwrap().status(), StatusCode::OK);
    fixture.issue_token().await;
    let (status, failure) = LinkFixture::json(browser.await.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["code"], "transport");
    assert_eq!(
        failure["message"],
        "The robot connector was disconnected before it answered."
    );
}

#[tokio::test]
async fn a_request_no_poll_took_fails_once_the_connector_stops_polling() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    let held = fixture.relay(
        Request::post(format!("{RELAY_PREFIX}/policies/start")),
        Body::empty(),
    );
    let delivered = poll.await.unwrap();
    let head: RelayedRequestHead =
        serde_json::from_slice(delivered.headers()[REQUEST_HEADER].as_bytes()).unwrap();
    // The connector stops polling, so no poll takes the next request.
    let queued = fixture.relay(
        Request::get(format!("{RELAY_PREFIX}/robots")),
        Body::empty(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Past the connector's linked window, only the request no poll took fails.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(36)).await;
    tokio::time::resume();
    let failed = tokio::time::timeout(Duration::from_secs(5), queued)
        .await
        .expect("the request fails without waiting for its answer");
    let (status, failure) = LinkFixture::json(failed.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["code"], "transport");
    assert_eq!(failure["message"], "The robot connector stopped answering");
    assert!(!held.is_finished());
    let answer = LinkFixture::connector(
        &token,
        Request::post(format!("{LINK_ANSWERS_PATH}/{}", head.id())),
    )
    .header(STATUS_HEADER, "202")
    .body(Body::empty())
    .unwrap();
    assert_eq!(fixture.send(answer).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(held.await.unwrap().status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn telemetry_is_watched_while_browsers_read_it() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    let read = || {
        fixture.relay(
            Request::get(format!("{RELAY_PREFIX}/robots/telemetry")),
            Body::empty(),
        )
    };
    let first = read();
    // The waiting poll learns at once that telemetry is watched.
    let polled = poll.await.unwrap();
    assert_eq!(polled.status(), StatusCode::NO_CONTENT);
    assert_eq!(LinkFixture::watched(&polled), [LiveStream::Telemetry]);
    let sample = r#"{"robots":[]}"#;
    let publish = LinkFixture::connector(&token, Request::post(LINK_TELEMETRY_PATH))
        .body(Body::from(sample))
        .unwrap();
    assert_eq!(fixture.send(publish).await.status(), StatusCode::NO_CONTENT);
    for response in [first.await.unwrap(), read().await.unwrap()] {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(&LinkFixture::body(response).await[..], sample.as_bytes());
    }

    // Unread for a lease, telemetry is unwatched, and its sample forgotten.
    let polled = fixture.poll(&token).await.unwrap();
    assert_eq!(polled.status(), StatusCode::NO_CONTENT);
    assert!(LinkFixture::watched(&polled).is_empty());
    let (status, failure) = LinkFixture::json(read().await.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["message"], "The robot connector sent no telemetry.");
}

#[tokio::test]
async fn a_camera_is_watched_while_its_mjpeg_response_is_open() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let poll = fixture.poll(&token);
    fixture.until_linked().await;
    let camera = "/dev/video0";
    let query = serde_urlencoded::to_string([
        ("id", camera),
        ("endpoint", ENDPOINT),
        ("project_id", PROJECT),
    ])
    .unwrap();
    // An `<img>` sends the session cookie and no other header.
    let request = Request::get(format!("{RELAY_PREFIX}/cameras/stream.mjpeg?{query}"))
        .header(header::COOKIE, fixture.project.cookie())
        .body(Body::empty())
        .unwrap();
    let stream = fixture.send(request).await;
    assert_eq!(stream.status(), StatusCode::OK);
    for (name, value) in [
        (
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace; boundary=spiral-robotics-frame",
        ),
        (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        (header::PRAGMA, "no-cache"),
        (header::EXPIRES, "0"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        assert_eq!(stream.headers()[&name], value, "{name}");
    }
    let polled = poll.await.unwrap();
    assert_eq!(
        LinkFixture::watched(&polled),
        [LiveStream::Camera {
            id: camera.to_owned()
        }]
    );

    let publish = |frame: &'static [u8]| {
        let query = serde_urlencoded::to_string([("id", camera)]).unwrap();
        LinkFixture::connector(
            &token,
            Request::post(format!("{LINK_CAMERAS_PATH}?{query}")),
        )
        .body(Body::from(frame))
        .unwrap()
    };
    let part = |frame: &[u8]| {
        let mut part = format!(
            "--spiral-robotics-frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            frame.len()
        )
        .into_bytes();
        part.extend_from_slice(frame);
        part.extend_from_slice(b"\r\n");
        part
    };
    let mut parts = stream.into_body().into_data_stream();
    let first: &[u8] = &[0xff, 0xd8, 0x01, 0xff, 0xd9];
    let second: &[u8] = &[0xff, 0xd8, 0x02, 0xff, 0xd9];
    assert_eq!(
        fixture.send(publish(first)).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(parts.next().await.unwrap().unwrap(), part(first));
    // A still scene posts the same picture again. It is still a new part: a
    // browser shows a frame only once the next one starts.
    fixture.send(publish(first)).await;
    assert_eq!(parts.next().await.unwrap().unwrap(), part(first));
    fixture.send(publish(second)).await;
    assert_eq!(parts.next().await.unwrap().unwrap(), part(second));

    // Closing the response unwatches the camera, and the next poll says so.
    drop(parts);
    let polled = fixture.poll(&token).await.unwrap();
    assert_eq!(polled.status(), StatusCode::NO_CONTENT);
    assert!(LinkFixture::watched(&polled).is_empty());
}

#[tokio::test]
async fn a_connector_uploads_an_episode_in_parts_that_then_waits_to_be_stored() {
    let fixture = LinkFixture::new().await;
    let token = fixture.issue_token().await;
    let upload = |token: &str, offset: u64, bytes: &'static [u8]| {
        let part = serde_urlencoded::to_string(EpisodePart::new(offset, 6)).unwrap();
        LinkFixture::connector(
            token,
            Request::put(format!("{LINK_EPISODES_PATH}/pick_place/episode-1?{part}")),
        )
        .body(Body::from(bytes))
        .unwrap()
    };
    let waiting = async || {
        let request = fixture
            .browser(Request::get(format!("{RELAY_PREFIX}/episodes")))
            .body(Body::empty())
            .unwrap();
        LinkFixture::json(fixture.send(request).await).await
    };

    assert_eq!(
        fixture.send(upload("rc_unknown", 0, b"abc")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        LinkFixture::json(fixture.send(upload(&token, 0, b"abc")).await).await,
        (StatusCode::OK, json!({"received": 3}))
    );
    // A part sent again says where the upload resumes.
    assert_eq!(
        LinkFixture::json(fixture.send(upload(&token, 0, b"abc")).await).await,
        (StatusCode::CONFLICT, json!({"received": 3}))
    );
    // Half an episode waits for nothing, and cannot be stored.
    assert_eq!(waiting().await, (StatusCode::OK, json!([])));
    let store = fixture
        .browser(Request::post(format!(
            "{RELAY_PREFIX}/episodes/pick_place/episode-1/store"
        )))
        .body(Body::empty())
        .unwrap();
    assert_eq!(fixture.send(store).await.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        LinkFixture::json(fixture.send(upload(&token, 3, b"def")).await).await,
        (StatusCode::OK, json!({"received": 6}))
    );
    assert_eq!(
        waiting().await,
        (
            StatusCode::OK,
            json!([{"dataset_id": "pick_place", "episode_id": "episode-1", "bytes": 6}])
        )
    );
}
