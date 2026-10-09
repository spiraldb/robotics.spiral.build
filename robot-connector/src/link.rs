//! The link the robot connector keeps to Studio, over plain HTTP.
//!
//! A robot rarely has a public address or a certificate, so the connector
//! dials out: it long-polls Studio for the next browser request to answer,
//! posts each answer back, while someone watches it posts telemetry and camera
//! snapshots, and it uploads each finished episode. Live data travels at display
//! rates; the control loop and teleoperation never wait on the link. The machine's [`LinkDriver`] sends
//! the requests — a computer's TLS stack or a microcontroller's — and
//! everything above it is shared.

mod episodes;
mod live;
mod relay;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{Request, Response, StatusCode, header};
use futures_util::future::BoxFuture;
use spiral_robotics_common::robot_connector::{
    HEALTH_HEADER, LINK_NEXT_PATH, LiveStream, REQUEST_HEADER, RelayedRequestHead,
    RobotConnectorHealth, WATCH_HEADER,
};

use crate::context::RobotConnectorContext;
use episodes::EpisodeUploads;
use live::LivePublishers;
use relay::RelayedRequest;

/// The first retry after a failed poll; each failure doubles it up to the cap.
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(30);

/// How one machine sends HTTP requests to Studio.
pub trait LinkDriver: Send + Sync {
    /// Send `request`, an absolute URL with its headers and body, and answer
    /// Studio's response. A long poll stays open for up to half a minute.
    fn send(&self, request: Request<Vec<u8>>) -> BoxFuture<'static, io::Result<Response<Vec<u8>>>>;
}

/// The Studio a connector links to, and the token its workspace issued.
pub struct StudioLink {
    origin: String,
    token: String,
}

impl StudioLink {
    /// Link to the Studio served at `studio`, its `http://` or `https://` web
    /// address, with `token`.
    pub fn new(studio: &str, token: impl Into<String>) -> Result<Self, String> {
        let origin = studio.trim().trim_end_matches('/');
        if !(origin.starts_with("https://") || origin.starts_with("http://")) {
            return Err(format!(
                "Studio's address must start with http:// or https://, not {origin:?}"
            ));
        }
        let token = token.into();
        if token.trim().is_empty() {
            return Err("a robot connector needs its workspace's token to link".to_owned());
        }
        Ok(Self {
            origin: origin.to_owned(),
            token,
        })
    }
}

/// Every request the connector makes of Studio, presenting its token.
#[derive(Clone)]
struct StudioClient {
    studio: Arc<StudioLink>,
    driver: Arc<dyn LinkDriver>,
}

impl StudioClient {
    /// Send `request`, whose URI is a path on Studio.
    async fn send(&self, request: Request<Vec<u8>>) -> io::Result<Response<Vec<u8>>> {
        let (mut parts, body) = request.into_parts();
        parts.uri = format!("{}{}", self.studio.origin, parts.uri)
            .parse()
            .map_err(io::Error::other)?;
        parts.headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", self.studio.token)
                .parse()
                .map_err(io::Error::other)?,
        );
        let response = self.driver.send(Request::from_parts(parts, body)).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(io::Error::other(
                "Studio refused this robot connector's token; issue a new one in Rollout → Set up",
            ));
        }
        Ok(response)
    }
}

/// What one poll brought: the streams to publish, and maybe a request.
struct Poll {
    watched: Vec<LiveStream>,
    request: Option<RelayedRequest>,
}

/// Keeps the connector linked: poll, hand the request off, poll again; and
/// uploads each finished episode meanwhile.
pub(crate) struct StudioLinkLoop {
    connector: RobotConnectorContext,
    api: Router,
    client: StudioClient,
}

impl StudioLinkLoop {
    pub(crate) fn new(
        connector: RobotConnectorContext,
        api: Router,
        studio: StudioLink,
        driver: Arc<dyn LinkDriver>,
    ) -> Self {
        Self {
            connector,
            api,
            client: StudioClient {
                studio: Arc::new(studio),
                driver,
            },
        }
    }

    /// Never returns; the caller stops it by dropping the future.
    pub(crate) async fn run(self) {
        let uploads = EpisodeUploads::new(
            self.connector.recording_files().clone(),
            self.client.clone(),
        );
        tokio::join!(self.poll_forever(), uploads.run());
    }

    async fn poll_forever(&self) {
        let mut live = LivePublishers::new(self.connector.clone(), self.client.clone());
        let mut linked = false;
        let mut retry = FIRST_RETRY;
        loop {
            match self.poll().await {
                Ok(Poll { watched, request }) => {
                    if !linked {
                        tracing::info!(studio = %self.client.studio.origin, "linked to Studio");
                        linked = true;
                    }
                    retry = FIRST_RETRY;
                    live.watch(watched);
                    if let Some(request) = request {
                        tokio::spawn(request.answer(self.api.clone(), self.client.clone()));
                    }
                }
                Err(error) => {
                    tracing::warn!(studio = %self.client.studio.origin, %error, "could not reach Studio");
                    linked = false;
                    live.watch(Vec::new());
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(LAST_RETRY);
                }
            }
        }
    }

    async fn poll(&self) -> io::Result<Poll> {
        let health =
            RobotConnectorHealth::new(env!("CARGO_PKG_VERSION"), cfg!(feature = "simulation"));
        let request = Request::get(LINK_NEXT_PATH)
            .header(
                HEALTH_HEADER,
                serde_json::to_string(&health).map_err(io::Error::other)?,
            )
            .body(Vec::new())
            .map_err(io::Error::other)?;
        let response = self.client.send(request).await?;
        let watched = match response.headers().get(WATCH_HEADER) {
            Some(watched) => {
                serde_json::from_slice(watched.as_bytes()).map_err(io::Error::other)?
            }
            None => Vec::new(),
        };
        let request = match response.status() {
            StatusCode::NO_CONTENT => None,
            StatusCode::OK => Some(RelayedRequest::from_poll(response)?),
            status => {
                return Err(io::Error::other(format!(
                    "Studio answered a poll with {status}"
                )));
            }
        };
        Ok(Poll { watched, request })
    }
}

impl RelayedRequest {
    /// The request a poll handed over: its head in a header, its body as the
    /// poll's.
    fn from_poll(response: Response<Vec<u8>>) -> io::Result<Self> {
        let (parts, body) = response.into_parts();
        let head: RelayedRequestHead = parts
            .headers
            .get(REQUEST_HEADER)
            .ok_or_else(|| io::Error::other("Studio handed over a request without its head"))
            .and_then(|head| serde_json::from_slice(head.as_bytes()).map_err(io::Error::other))?;
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        Ok(Self::new(head, content_type, body))
    }
}

#[cfg(test)]
mod tests;
