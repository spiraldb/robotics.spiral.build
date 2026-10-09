//! The live streams Studio is watching, each posted by its own task while it
//! is watched.
//!
//! Live data is for display, so it travels at display rates and latest-wins:
//! each post carries the newest sample, and anything that arrived meanwhile
//! is skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use axum::http::{Request, header};
use spiral_robotics_common::robot_connector::{LINK_CAMERAS_PATH, LINK_TELEMETRY_PATH, LiveStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::StudioClient;
use crate::context::RobotConnectorContext;

/// Enough for a smooth joint readout and 3D arm in the browser.
const TELEMETRY_PERIOD: Duration = Duration::from_millis(100);
/// A preview to frame and check a camera by; episodes record every frame.
const CAMERA_PERIOD: Duration = Duration::from_millis(200);

pub(super) struct LivePublishers {
    connector: RobotConnectorContext,
    client: StudioClient,
    running: BTreeMap<LiveStream, JoinHandle<()>>,
}

impl LivePublishers {
    pub(super) fn new(connector: RobotConnectorContext, client: StudioClient) -> Self {
        Self {
            connector,
            client,
            running: BTreeMap::new(),
        }
    }

    /// Publish exactly `streams` from now on.
    pub(super) fn watch(&mut self, streams: Vec<LiveStream>) {
        let wanted = streams.into_iter().collect::<BTreeSet<_>>();
        self.running.retain(|stream, task| {
            let keep = wanted.contains(stream);
            if !keep {
                task.abort();
            }
            keep
        });
        for stream in wanted {
            if self.running.contains_key(&stream) {
                continue;
            }
            let publisher = LivePublisher {
                connector: self.connector.clone(),
                client: self.client.clone(),
                stream: stream.clone(),
            };
            self.running.insert(stream, tokio::spawn(publisher.run()));
        }
    }
}

impl Drop for LivePublishers {
    fn drop(&mut self) {
        for task in self.running.values() {
            task.abort();
        }
    }
}

struct LivePublisher {
    connector: RobotConnectorContext,
    client: StudioClient,
    stream: LiveStream,
}

impl LivePublisher {
    async fn run(self) {
        match &self.stream {
            LiveStream::Telemetry => self.publish_telemetry().await,
            LiveStream::Camera { id } => self.publish_camera(id).await,
        }
    }

    async fn publish_telemetry(&self) {
        let mut states = self.connector.hardware().subscribe();
        while states.changed().await.is_ok() {
            let next = Instant::now() + TELEMETRY_PERIOD;
            // The sample is taken before the post, so the state's lock is not
            // held across it.
            let sample = serde_json::to_vec(states.borrow_and_update().telemetry());
            match sample {
                Ok(sample) => {
                    self.post(LINK_TELEMETRY_PATH.to_owned(), "application/json", sample)
                        .await
                }
                Err(error) => tracing::warn!("could not encode telemetry: {error}"),
            }
            tokio::time::sleep_until(next).await;
        }
    }

    async fn publish_camera(&self, id: &str) {
        let mut frames = match self.connector.cameras().subscribe(id) {
            Ok(frames) => frames,
            Err(error) => {
                tracing::warn!(camera = %id, "cannot publish a camera Studio watches: {error}");
                return;
            }
        };
        let path = format!(
            "{LINK_CAMERAS_PATH}?id={}",
            form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
        );
        let mut last_sequence = 0;
        while frames.changed().await.is_ok() {
            let next = Instant::now() + CAMERA_PERIOD;
            let Some(frame) = frames.borrow_and_update().clone() else {
                continue;
            };
            if frame.sequence() == last_sequence {
                continue;
            }
            last_sequence = frame.sequence();
            self.post(path.clone(), "image/jpeg", frame.bytes().to_vec())
                .await;
            tokio::time::sleep_until(next).await;
        }
    }

    async fn post(&self, path: String, content_type: &'static str, sample: Vec<u8>) {
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, content_type)
            .body(sample);
        let sent = match request {
            Ok(request) => self.client.send(request).await.map(|_| ()),
            Err(error) => Err(std::io::Error::other(error)),
        };
        // The poll loop notices a lost link; a missed sample is superseded.
        if let Err(error) = sent {
            tracing::debug!(stream = ?self.stream, %error, "a live sample did not reach Studio");
        }
    }
}
