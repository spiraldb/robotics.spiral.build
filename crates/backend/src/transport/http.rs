use crate::http::{
    contract::{ApiRoutes, OperationSchema, RequestSchema, ResponseSchema},
    delivery::HttpFailure,
};
use crate::project_session::context::ProjectContext;
use axum::{
    Json,
    body::Body,
    extract::{FromRequest, State},
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt as _;
use http_body::Body as _;
use reqwest::Url;
use serde_json::{Value, json};
use spiral_robotics_common::credentials::StudioCredentials;
use std::{str::FromStr, time::Duration};

pub(crate) struct TransportEndpoints;

impl TransportEndpoints {
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/credentials", Self::credentials);
        routes.get("/proxy", Self::proxy);
        routes.post("/proxy", Self::proxy);
        routes.put("/proxy", Self::proxy);
        routes.patch("/proxy", Self::proxy);
        routes.delete("/proxy", Self::proxy);
        routes.head("/proxy", Self::proxy);
        routes.options("/proxy", Self::proxy);
    }

    async fn credentials(
        State(state): State<ProjectContext>,
    ) -> Result<Json<StudioCredentials>, HttpFailure> {
        super::credentials::ConfiguredCredentials::load(state.backend_state())
            .map(Json)
            .map_err(|error| crate::OperationError::External(error).into())
    }

    async fn proxy(request: ProxyRequest) -> Result<ProxyResponse, HttpFailure> {
        ProxyRelay::new()?.send(request).await
    }
}

struct ProxyRelay {
    client: reqwest::Client,
}
impl ProxyRelay {
    fn new() -> Result<Self, HttpFailure> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map(|client| Self { client })
            .map_err(|error| {
                crate::OperationError::External(format!("Could not configure HTTP relay: {error}"))
                    .into()
            })
    }

    async fn send(&self, request: ProxyRequest) -> Result<ProxyResponse, HttpFailure> {
        let bodyless = request.body.is_end_stream();
        let mut upstream = self
            .client
            .request(request.method, request.url)
            .headers(request.headers);
        if !bodyless {
            upstream = upstream.body(reqwest::Body::wrap_stream(request.body.into_data_stream()));
        }
        let response = upstream.send().await.map_err(|error| {
            HttpFailure::from(crate::OperationError::External(format!(
                "Upstream HTTP request failed: {error}"
            )))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        Ok(ProxyResponse {
            status,
            headers,
            body: Body::from_stream(stream),
        })
    }
}

struct ProxyRequest {
    url: Url,
    method: Method,
    headers: HeaderMap,
    body: Body,
}

impl<State: Send + Sync> FromRequest<State> for ProxyRequest {
    type Rejection = HttpFailure;
    async fn from_request(
        request: axum::extract::Request,
        _: &State,
    ) -> Result<Self, Self::Rejection> {
        let target: ProxyTarget =
            serde_urlencoded::from_str(request.uri().query().unwrap_or_default())
                .map_err(|_| HttpFailure::invalid("Proxy url query parameter is required"))?;
        let url = Url::parse(&target.url)
            .map_err(|error| HttpFailure::invalid(format!("Invalid upstream URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(HttpFailure::invalid(
                "Upstream URL must be an http:// or https:// URL without embedded credentials",
            ));
        }
        let method = request.method().clone();
        let mut headers = HeaderMap::new();
        let nominated = connection_headers(request.headers());
        for (name, value) in request.headers() {
            if !forbidden_request_header(name) && !nominated.contains(name) {
                headers.append(name.clone(), value.clone());
            }
        }
        Ok(Self {
            url,
            method,
            headers,
            body: request.into_body(),
        })
    }
}

#[derive(serde::Deserialize)]
struct ProxyTarget {
    url: String,
}

fn forbidden_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "accept-encoding"
            | "accept-language"
            | "content-length"
            | "cookie"
            | "dnt"
            | "forwarded"
            | "host"
            | "keep-alive"
            | "origin"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "priority"
            | "referer"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "user-agent"
            | "via"
            | "x-project-session"
    ) || name.as_str().starts_with("sec-")
        || name.as_str().starts_with("x-forwarded-")
}

impl RequestSchema for ProxyRequest {
    fn describe(operation: &mut OperationSchema, _: &mut schemars::SchemaGenerator) {
        operation.proxy();
    }
}

struct ProxyResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
}
impl IntoResponse for ProxyResponse {
    fn into_response(self) -> Response {
        let mut response = Response::new(self.body);
        *response.status_mut() = self.status;
        let nominated = connection_headers(&self.headers);
        for (name, value) in &self.headers {
            if !forbidden_response_header(name) && !nominated.contains(name) {
                response.headers_mut().append(name.clone(), value.clone());
            }
        }
        response
    }
}
fn forbidden_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
fn connection_headers(headers: &HeaderMap) -> Vec<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_str(name.trim()).ok())
        .collect()
}
impl ResponseSchema for Result<ProxyResponse, HttpFailure> {
    fn responses(_: &mut schemars::SchemaGenerator) -> Value {
        json!({
            "default": {"description":"Raw upstream response preserving status, end-to-end headers, and streaming body. Relay failures use a JSON CommandFailure when no upstream response exists."}
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use axum::{
        Router,
        body::{Bytes, to_bytes},
        extract::State,
        http::Request,
        routing::{any, get},
    };
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    #[derive(Clone)]
    struct UpstreamState {
        base: String,
        cancellation: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    }

    async fn echo(method: Method, headers: HeaderMap, body: Bytes) -> Response {
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = StatusCode::MULTI_STATUS;
        response
            .headers_mut()
            .insert("x-method", HeaderValue::from_str(method.as_str()).unwrap());
        for value in headers.get_all("x-input").iter() {
            response.headers_mut().append("x-echo", value.clone());
        }
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("x-hop"));
        response
            .headers_mut()
            .insert("x-hop", HeaderValue::from_static("remove"));
        response
    }

    async fn redirect(State(state): State<UpstreamState>) -> Response {
        Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, format!("{}/target", state.base))
            .body(Body::empty())
            .unwrap()
    }

    async fn target(method: Method) -> Response {
        Response::new(Body::from(format!("redirected:{method}")))
    }

    async fn head() -> Response {
        Response::builder()
            .header(header::CONTENT_LENGTH, "99")
            .header("x-head", "yes")
            .body(Body::empty())
            .unwrap()
    }

    struct CancellationGuard(Option<oneshot::Sender<()>>);
    impl Drop for CancellationGuard {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    async fn cancellable(State(state): State<UpstreamState>) -> Response {
        let guard = CancellationGuard(state.cancellation.lock().unwrap().take());
        let stream = futures_util::stream::unfold(guard, |guard| async move {
            std::future::pending::<()>().await;
            Some((Ok::<Bytes, std::io::Error>(Bytes::new()), guard))
        });
        Response::new(Body::from_stream(stream))
    }

    async fn upstream() -> (String, tokio::task::JoinHandle<()>, oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = oneshot::channel();
        let state = UpstreamState {
            base: base.clone(),
            cancellation: Arc::new(Mutex::new(Some(sender))),
        };
        let app = Router::new()
            .route("/echo", any(echo))
            .route("/redirect", get(redirect))
            .route("/target", get(target))
            .route("/head", any(head))
            .route("/cancel", get(cancellable))
            .with_state(state);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, server, receiver)
    }

    fn request(url: String, method: Method, headers: HeaderMap, body: Body) -> ProxyRequest {
        ProxyRequest {
            url: Url::parse(&url).unwrap(),
            method,
            headers,
            body,
        }
    }

    #[tokio::test]
    async fn relay_preserves_method_headers_status_binary_and_redirects() {
        let (base, server, _cancelled) = upstream().await;
        let relay = ProxyRelay::new().unwrap();
        let mut headers = HeaderMap::new();
        headers.append("x-input", HeaderValue::from_static("one"));
        headers.append("x-input", HeaderValue::from_static("two"));
        let binary = Bytes::from_static(&[0, 159, 146, 150, 255]);
        let response = relay
            .send(request(
                format!("{base}/echo"),
                Method::PATCH,
                headers,
                Body::from(binary.clone()),
            ))
            .await
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        assert_eq!(response.headers()["x-method"], "PATCH");
        assert_eq!(response.headers().get_all("x-echo").iter().count(), 2);
        assert!(response.headers().get("x-hop").is_none());
        assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), binary);

        let response = relay
            .send(request(
                format!("{base}/redirect"),
                Method::GET,
                HeaderMap::new(),
                Body::empty(),
            ))
            .await
            .unwrap()
            .into_response();
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "redirected:GET"
        );

        let response = relay
            .send(request(
                format!("{base}/head"),
                Method::HEAD,
                HeaderMap::new(),
                Body::empty(),
            ))
            .await
            .unwrap()
            .into_response();
        assert_eq!(response.headers()["x-head"], "yes");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "99");
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
        server.abort();
    }

    #[tokio::test]
    async fn dropping_relay_body_cancels_upstream_transport() {
        let (base, server, cancelled) = upstream().await;
        let response = ProxyRelay::new()
            .unwrap()
            .send(request(
                format!("{base}/cancel"),
                Method::GET,
                HeaderMap::new(),
                Body::empty(),
            ))
            .await
            .unwrap();
        drop(response);
        tokio::time::timeout(Duration::from_secs(1), cancelled)
            .await
            .expect("upstream body should be dropped")
            .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn request_extractor_strips_browser_headers_and_keeps_raw_body() {
        let raw = Bytes::from_static(&[0, 1, 2, 255]);
        let request = Request::builder()
            .method(Method::PUT)
            .uri("/api/proxy?url=https%3A%2F%2Fexample.com%2Fupload")
            .header("content-type", "application/octet-stream")
            .header("cookie", "studio=session")
            .header("x-project-session", "project")
            .body(Body::from(raw.clone()))
            .unwrap();
        let extracted = ProxyRequest::from_request(request, &()).await.unwrap();
        assert_eq!(extracted.method, Method::PUT);
        assert_eq!(
            extracted.headers[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert!(extracted.headers.get(header::COOKIE).is_none());
        assert!(extracted.headers.get("x-project-session").is_none());
        assert_eq!(to_bytes(extracted.body, 1024).await.unwrap(), raw);
    }
}
