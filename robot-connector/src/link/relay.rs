//! A browser request Studio relayed, answered by the connector's own API.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use spiral_robotics_common::robot_connector::{
    LINK_ANSWERS_PATH, RelayedRequestHead, STATUS_HEADER,
};
use tower::ServiceExt as _;

use super::StudioClient;

pub(super) struct RelayedRequest {
    head: RelayedRequestHead,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// What the connector's API answered.
struct RelayedAnswer {
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl RelayedRequest {
    pub(super) fn new(
        head: RelayedRequestHead,
        content_type: Option<String>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            head,
            content_type,
            body,
        }
    }

    /// Answer through `api`, then post the answer back to Studio.
    pub(super) async fn answer(self, api: Router, client: StudioClient) {
        let id = self.head.id();
        let answer = match self.into_request() {
            Ok(request) => RelayedAnswer::from_api(api, request).await,
            Err(error) => RelayedAnswer::plain(
                StatusCode::BAD_REQUEST,
                format!("Studio relayed a request the robot connector cannot read: {error}"),
            ),
        };
        if let Err(error) = answer.post(id, &client).await {
            // Studio fails the browser's request when its answer never comes.
            tracing::warn!(request = id, %error, "could not answer a relayed request");
        }
    }

    fn into_request(self) -> Result<Request<Body>, axum::http::Error> {
        let mut request = Request::builder()
            .method(self.head.method())
            .uri(self.head.path());
        if let Some(content_type) = &self.content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        request.body(Body::from(self.body))
    }
}

impl RelayedAnswer {
    async fn from_api(api: Router, request: Request<Body>) -> Self {
        let response = api
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        match axum::body::to_bytes(response.into_body(), usize::MAX).await {
            Ok(body) => Self {
                status,
                content_type,
                body: body.to_vec(),
            },
            Err(error) => Self::plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not read the response: {error}"),
            ),
        }
    }

    fn plain(status: StatusCode, message: String) -> Self {
        Self {
            status: status.as_u16(),
            content_type: None,
            body: message.into_bytes(),
        }
    }

    async fn post(self, id: u64, client: &StudioClient) -> std::io::Result<()> {
        let mut request =
            Request::post(format!("{LINK_ANSWERS_PATH}/{id}")).header(STATUS_HEADER, self.status);
        if let Some(content_type) = &self.content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let request = request.body(self.body).map_err(std::io::Error::other)?;
        client.send(request).await.map(|_| ())
    }
}
