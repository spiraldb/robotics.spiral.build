use crate::http::delivery::HttpFailure;
use crate::projects::context::StudioContext;
use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequest, State},
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use futures_util::TryStreamExt as _;
use http_body::Body as _;
use reqwest::Url;
use spiral_robotics_common::providers::{ProviderKeysStatus, ProviderKeysUpdate, StudioProviders};
use std::{str::FromStr, time::Duration};

use super::credentials::ProviderCredentials;

pub(crate) struct TransportEndpoints;

impl TransportEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/providers", get(Self::providers))
            .route("/keys", get(Self::keys))
            .route("/proxy", get(Self::proxy).post(Self::proxy))
    }

    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new().route("/keys", put(Self::update_keys))
    }

    async fn providers(
        State(state): State<StudioContext>,
    ) -> Result<Json<StudioProviders>, HttpFailure> {
        ProviderCredentials::load(&state.keys().current())
            .providers()
            .map(Json)
            .map_err(|error| crate::OperationError::External(error).into())
    }

    /// Relay a browser request, authenticated with the key Studio holds for
    /// its destination.
    async fn proxy(
        State(state): State<StudioContext>,
        request: ProxyRequest,
    ) -> Result<ProxyResponse, HttpFailure> {
        let credentials = ProviderCredentials::load(&state.keys().current());
        let bearer = credentials.bearer_for(&request.url);
        ProxyRelay::new()?.send(request.with_bearer(bearer)).await
    }

    /// Which keys are saved, never the keys.
    async fn keys(State(state): State<StudioContext>) -> Json<ProviderKeysStatus> {
        Json(state.keys().current().status())
    }

    /// Save new keys. Requests and database connections made after the save
    /// use them; open projects keep running.
    async fn update_keys(
        State(state): State<StudioContext>,
        Json(update): Json<ProviderKeysUpdate>,
    ) -> Result<Json<ProviderKeysStatus>, HttpFailure> {
        state
            .keys()
            .update(&update)
            .await
            .map(|keys| Json(keys.status()))
            .map_err(|error| crate::OperationError::Internal(error).into())
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

impl ProxyRequest {
    fn with_bearer(mut self, token: Option<&str>) -> Self {
        if let Some(token) = token
            && let Ok(mut value) = header::HeaderValue::from_str(&format!("Bearer {token}"))
        {
            value.set_sensitive(true);
            self.headers.insert(header::AUTHORIZATION, value);
        }
        self
    }

    /// The relay attaches Studio's keys, so only Studio's own pages may use it:
    /// another site open in the same browser must not send requests through it.
    /// Callers outside a browser send neither header.
    fn from_studio_page(headers: &HeaderMap) -> bool {
        if let Some(site) = headers.get("sec-fetch-site") {
            return matches!(site.to_str(), Ok("same-origin" | "none"));
        }
        let Some(origin) = headers.get(header::ORIGIN) else {
            return true;
        };
        let origin = origin
            .to_str()
            .ok()
            .and_then(|origin| Url::parse(origin).ok());
        let host = headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok());
        origin.zip(host).is_some_and(|(origin, host)| {
            let authority = match (origin.host_str(), origin.port()) {
                (Some(name), Some(port)) => format!("{name}:{port}"),
                (Some(name), None) => name.to_owned(),
                (None, _) => return false,
            };
            authority == host
        })
    }
}

impl<State: Send + Sync> FromRequest<State> for ProxyRequest {
    type Rejection = HttpFailure;
    async fn from_request(
        request: axum::extract::Request,
        _: &State,
    ) -> Result<Self, Self::Rejection> {
        if !Self::from_studio_page(request.headers()) {
            return Err(HttpFailure::rejection(
                StatusCode::FORBIDDEN,
                "Only Studio's own pages may use its HTTP relay",
            ));
        }
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
            | "x-spiral-endpoint"
            | "x-spiral-project"
    ) || name.as_str().starts_with("sec-")
        || name.as_str().starts_with("x-forwarded-")
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
                Method::POST,
                headers,
                Body::from(binary.clone()),
            ))
            .await
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        assert_eq!(response.headers()["x-method"], "POST");
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
    async fn relay_refuses_other_sites_and_attaches_the_destinations_key() {
        let cross_site = |header: &'static str, value: &'static str| {
            Request::builder()
                .uri("/api/proxy?url=https%3A%2F%2Fopenrouter.ai%2Fapi%2Fv1")
                .header("host", "127.0.0.1:8080")
                .header(header, value)
                .body(Body::empty())
                .unwrap()
        };
        for (header, value) in [
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-site", "same-site"),
            ("origin", "https://attacker.example"),
            ("origin", "http://127.0.0.1:9999"),
        ] {
            let rejected = ProxyRequest::from_request(cross_site(header, value), &()).await;
            assert!(rejected.is_err(), "{header}: {value}");
        }
        for (header, value) in [
            ("sec-fetch-site", "same-origin"),
            ("origin", "http://127.0.0.1:8080"),
        ] {
            let accepted = ProxyRequest::from_request(cross_site(header, value), &())
                .await
                .unwrap();
            let request = accepted.with_bearer(Some("provider-key"));
            assert_eq!(
                request.headers[header::AUTHORIZATION],
                "Bearer provider-key"
            );
            assert!(request.headers[header::AUTHORIZATION].is_sensitive());
        }
    }

    #[tokio::test]
    async fn request_extractor_strips_browser_headers_and_keeps_raw_body() {
        let raw = Bytes::from_static(&[0, 1, 2, 255]);
        let request = Request::builder()
            .method(Method::PUT)
            .uri("/api/proxy?url=https%3A%2F%2Fexample.com%2Fupload")
            .header("content-type", "application/octet-stream")
            .header("cookie", "studio=session")
            .header("x-spiral-project", "project")
            .body(Body::from(raw.clone()))
            .unwrap();
        let extracted = ProxyRequest::from_request(request, &()).await.unwrap();
        assert_eq!(extracted.method, Method::PUT);
        assert_eq!(
            extracted.headers[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert!(extracted.headers.get(header::COOKIE).is_none());
        assert!(extracted.headers.get("x-spiral-project").is_none());
        assert_eq!(to_bytes(extracted.body, 1024).await.unwrap(), raw);
    }
}
