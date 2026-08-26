//! Typed HTTP and event-stream transport to the Studio backend.
//!
//! Requests go to the same origin the page was served from, so the UI works
//! unchanged whether the studio is on localhost or on the robot's own machine.

use std::collections::BTreeMap;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
use futures_util::{StreamExt, stream};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::{Method, RequestBuilder};
use js_sys::Uint8Array;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spiral_robotics_api::recording::EpisodeInfo;
pub(crate) use spiral_robotics_api::recording::{
    EpisodeInspectionSnapshot, EpisodeVideoAssetMetadata as VideoAssetMetadata,
};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::robots::TelemetryFrame;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::rollout::RolloutSnapshot;
use spiral_robotics_api::status::ApiError;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::status::StudioSnapshot;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::training::{TrainingSnapshot, TrainingUpdate};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::workflow::{WorkflowAgentStateResponse, WorkflowStateResponse};

pub(crate) struct Http;

impl Http {
    /// Fetch and decode a JSON resource.
    pub(crate) async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
        RawRequest::new(Method::GET, path)
            .send()
            .await?
            .decode_json(path)
    }

    /// Send a request whose response body does not matter.
    pub(crate) async fn send<B: Serialize>(
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), String> {
        let body = body.map(RequestBody::request_value).transpose()?;
        RawRequest::new(method, path)
            .with_body(body)
            .send()
            .await
            .map(|_| ())
    }

    /// Send a request and decode its JSON response.
    pub(crate) async fn send_json<B: Serialize, T: DeserializeOwned>(
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, String> {
        let body = body.map(RequestBody::request_value).transpose()?;
        RawRequest::new(method, path)
            .with_body(body)
            .send()
            .await?
            .decode_json(path)
    }

    /// POST with no request body.
    pub(crate) async fn post(path: &str) -> Result<(), String> {
        Self::send::<()>(Method::POST, path, None).await
    }
}

/// One JSON request with an optional browser-owned cancellation signal.
pub(crate) struct AbortableJsonRequest<'a, B> {
    method: Method,
    path: &'a str,
    body: Option<&'a B>,
    signal: Option<&'a web_sys::AbortSignal>,
}

impl<'a, B: Serialize> AbortableJsonRequest<'a, B> {
    pub(crate) fn new(method: Method, path: &'a str, body: Option<&'a B>) -> Self {
        Self {
            method,
            path,
            body,
            signal: None,
        }
    }

    pub(crate) fn with_signal(mut self, signal: Option<&'a web_sys::AbortSignal>) -> Self {
        self.signal = signal;
        self
    }

    /// Aborting releases the UI immediately; hosted database work is isolated on
    /// its own checked-out connection, so an abandoned query cannot block
    /// persistence or replay.
    pub(crate) async fn send<T: DeserializeOwned>(self) -> Result<T, String> {
        let body = self.body.map(RequestBody::request_value).transpose()?;
        let response = RawRequest::new(self.method, self.path)
            .with_body(body)
            .with_signal(self.signal)
            .send()
            .await?;
        response.decode_json(self.path)
    }
}

/// Episode detail used to align timeline samples and recorded media.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct EpisodeTimelineResponse {
    episode: EpisodeInfo,
    videos: Vec<VideoAssetMetadata>,
}

impl EpisodeTimelineResponse {
    pub(crate) const fn new(episode: EpisodeInfo, videos: Vec<VideoAssetMetadata>) -> Self {
        Self { episode, videos }
    }

    pub(crate) const fn episode(&self) -> &EpisodeInfo {
        &self.episode
    }

    pub(crate) fn videos(&self) -> &[VideoAssetMetadata] {
        &self.videos
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct TimelineArmSample {
    arm: String,
    normalized: BTreeMap<String, Option<f64>>,
    degrees: BTreeMap<String, Option<f64>>,
    velocity: Option<BTreeMap<String, Option<f64>>>,
    effort: Option<BTreeMap<String, Option<f64>>>,
    action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TimelineOptionalArmChannels {
    velocity: Option<BTreeMap<String, Option<f64>>>,
    effort: Option<BTreeMap<String, Option<f64>>>,
    action: Option<BTreeMap<String, Option<f64>>>,
}

impl TimelineOptionalArmChannels {
    pub(crate) const fn new(
        velocity: Option<BTreeMap<String, Option<f64>>>,
        effort: Option<BTreeMap<String, Option<f64>>>,
        action: Option<BTreeMap<String, Option<f64>>>,
    ) -> Self {
        Self {
            velocity,
            effort,
            action,
        }
    }
}

impl TimelineArmSample {
    pub(crate) fn new(
        arm: impl Into<String>,
        normalized: BTreeMap<String, Option<f64>>,
        degrees: BTreeMap<String, Option<f64>>,
    ) -> Self {
        Self {
            arm: arm.into(),
            normalized,
            degrees,
            velocity: None,
            effort: None,
            action: None,
        }
    }

    pub(crate) fn with_optional_channels(mut self, channels: TimelineOptionalArmChannels) -> Self {
        self.velocity = channels.velocity;
        self.effort = channels.effort;
        self.action = channels.action;
        self
    }

    pub(crate) fn arm(&self) -> &str {
        &self.arm
    }

    pub(crate) const fn normalized(&self) -> &BTreeMap<String, Option<f64>> {
        &self.normalized
    }

    pub(crate) const fn degrees(&self) -> &BTreeMap<String, Option<f64>> {
        &self.degrees
    }
}

/// The nearest stored sample to a committed recorded playhead position.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct TimelineSample {
    frame: u64,
    seconds: f64,
    follow_error_degrees: Option<f64>,
    arms: Vec<TimelineArmSample>,
}

impl TimelineSample {
    pub(crate) const fn new(frame: u64, seconds: f64) -> Self {
        Self {
            frame,
            seconds,
            follow_error_degrees: None,
            arms: Vec::new(),
        }
    }

    pub(crate) fn with_measurements(
        mut self,
        follow_error_degrees: Option<f64>,
        arms: Vec<TimelineArmSample>,
    ) -> Self {
        self.follow_error_degrees = follow_error_degrees;
        self.arms = arms;
        self
    }

    #[cfg(test)]
    pub(crate) const fn frame(&self) -> u64 {
        self.frame
    }

    pub(crate) const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub(crate) fn arms(&self) -> &[TimelineArmSample] {
        &self.arms
    }
}

impl Http {
    /// Upload one file and decode the server's JSON response.
    pub(crate) async fn upload_json<T: DeserializeOwned>(
        path: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<T, String> {
        let body = Uint8Array::from(bytes);
        let response = RequestBuilder::new(path)
            .method(Method::POST)
            .header("Content-Type", content_type)
            .body(body)
            .map_err(|error| format!("could not build the upload: {error}"))?
            .send()
            .await
            .map_err(|error| format!("the studio server is unreachable: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if (200..300).contains(&status) {
            return text.decode_json(path);
        }
        Err(serde_json::from_str::<ApiError>(&text)
            .map(ApiError::into_error)
            .unwrap_or_else(|_| format!("upload to {path} failed with status {status}")))
    }

    /// Upload the browser's native File object directly through Fetch. This avoids
    /// copying a potentially hundreds-of-megabytes archive through Rust/WASM and
    /// a second JavaScript Uint8Array before the network request can begin.
    pub(crate) async fn upload_file_json<T: DeserializeOwned>(
        path: &str,
        file: &dioxus::html::FileData,
        content_type: &str,
    ) -> Result<T, String> {
        #[cfg(target_arch = "wasm32")]
        let request = {
            let web_file = file
                .inner()
                .downcast_ref::<web_sys::File>()
                .cloned()
                .ok_or_else(|| format!("{} is not a browser file", file.name()))?;
            RequestBuilder::new(path)
                .method(Method::POST)
                .header("Content-Type", content_type)
                .body(web_file)
                .map_err(|error| format!("could not build the upload: {error}"))?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let request = {
            let bytes = file
                .read_bytes()
                .await
                .map_err(|error| format!("could not read {}: {error}", file.name()))?;
            RequestBuilder::new(path)
                .method(Method::POST)
                .header("Content-Type", content_type)
                .body(Uint8Array::from(bytes.as_ref()))
                .map_err(|error| format!("could not build the upload: {error}"))?
        };
        let response = request
            .send()
            .await
            .map_err(|error| format!("the studio server is unreachable: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if (200..300).contains(&status) {
            return text.decode_json(path);
        }
        Err(serde_json::from_str::<ApiError>(&text)
            .map(ApiError::into_error)
            .unwrap_or_else(|_| format!("upload to {path} failed with status {status}")))
    }
}

trait RequestBody {
    fn request_value(&self) -> Result<Value, String>;
}

impl<B: Serialize> RequestBody for B {
    fn request_value(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|error| format!("could not encode the request: {error}"))
    }
}

trait ResponseBody {
    fn decode_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String>;
}

impl ResponseBody for str {
    fn decode_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        serde_json::from_str(self)
            .map_err(|error| format!("could not read the response from {path}: {error}"))
    }
}

struct RawRequest<'a> {
    method: Method,
    path: &'a str,
    body: Option<Value>,
    signal: Option<&'a web_sys::AbortSignal>,
}

impl<'a> RawRequest<'a> {
    fn new(method: Method, path: &'a str) -> Self {
        Self {
            method,
            path,
            body: None,
            signal: None,
        }
    }

    fn with_body(mut self, body: Option<Value>) -> Self {
        self.body = body;
        self
    }

    fn with_signal(mut self, signal: Option<&'a web_sys::AbortSignal>) -> Self {
        self.signal = signal;
        self
    }

    async fn send(self) -> Result<String, String> {
        let builder = RequestBuilder::new(self.path)
            .method(self.method)
            .abort_signal(self.signal);
        let request = match self.body {
            Some(value) => builder.json(&value),
            None => builder.build(),
        }
        .map_err(|error| format!("could not build the request: {error}"))?;

        let response = request
            .send()
            .await
            .map_err(|error| format!("the studio server is unreachable: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if (200..300).contains(&status) {
            return Ok(text);
        }
        // The server always describes a failure in the body; fall back to the
        // status only when something else answered.
        Err(serde_json::from_str::<ApiError>(&text)
            .map(ApiError::into_error)
            .unwrap_or_else(|_| format!("request to {} failed with status {status}", self.path)))
    }
}

/// One message from the studio's event stream.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub(crate) enum StudioEvent {
    Snapshot(Box<StudioSnapshot>),
    Telemetry(Box<TelemetryFrame>),
    TrainingSnapshot(Box<TrainingSnapshot>),
    TrainingUpdate(Box<TrainingUpdate>),
    Rollout(Box<RolloutSnapshot>),
    WorkflowState(Box<WorkflowStateResponse>),
    WorkflowAgent(Box<WorkflowAgentStateResponse>),
}

/// Consume the server's event stream, handing every message to `on_event`.
///
/// Returns when the connection closes, which the caller treats as a signal to
/// reconnect. The [`EventSource`] is owned by this future, so dropping the
/// future closes the connection.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
impl Http {
    pub(crate) async fn stream_events<F>(mut on_event: F) -> Result<(), String>
    where
        F: FnMut(StudioEvent),
    {
        let mut source = EventSource::new("/api/events")
            .map_err(|error| format!("could not open the event stream: {error}"))?;
        let snapshots = source
            .subscribe("state")
            .map_err(|error| format!("could not subscribe to studio state: {error}"))?;
        let telemetry = source
            .subscribe("telemetry")
            .map_err(|error| format!("could not subscribe to telemetry: {error}"))?;
        let training_snapshots = source
            .subscribe("training_snapshot")
            .map_err(|error| format!("could not subscribe to training snapshots: {error}"))?;
        let training_updates = source
            .subscribe("training_update")
            .map_err(|error| format!("could not subscribe to training updates: {error}"))?;
        let rollout = source
            .subscribe("rollout_state")
            .map_err(|error| format!("could not subscribe to rollout state: {error}"))?;
        let workflow_snapshots = source
            .subscribe("workflow_snapshot")
            .map_err(|error| format!("could not subscribe to Workflow snapshots: {error}"))?;
        let workflow_updates = source
            .subscribe("workflow_update")
            .map_err(|error| format!("could not subscribe to Workflow updates: {error}"))?;
        let workflow_agent = source.subscribe("workflow_agent").map_err(|error| {
            format!("could not subscribe to Workflow Assistant updates: {error}")
        })?;

        let mut events = stream::select(
            stream::select(stream::select(snapshots, telemetry), rollout),
            stream::select(
                stream::select(training_snapshots, training_updates),
                stream::select(
                    stream::select(workflow_snapshots, workflow_updates),
                    workflow_agent,
                ),
            ),
        );
        while let Some(Ok((kind, message))) = events.next().await {
            let Some(text) = message.data().as_string() else {
                continue;
            };
            match kind.as_str() {
                "state" => {
                    if let Ok(snapshot) = serde_json::from_str(&text) {
                        on_event(StudioEvent::Snapshot(Box::new(snapshot)));
                    }
                }
                "telemetry" => {
                    if let Ok(frame) = serde_json::from_str(&text) {
                        on_event(StudioEvent::Telemetry(Box::new(frame)));
                    }
                }
                "training_snapshot" => {
                    if let Ok(snapshot) = serde_json::from_str(&text) {
                        on_event(StudioEvent::TrainingSnapshot(Box::new(snapshot)));
                    }
                }
                "training_update" => {
                    if let Ok(update) = serde_json::from_str(&text) {
                        on_event(StudioEvent::TrainingUpdate(Box::new(update)));
                    }
                }
                "rollout_state" => {
                    if let Ok(snapshot) = serde_json::from_str(&text) {
                        on_event(StudioEvent::Rollout(Box::new(snapshot)));
                    }
                }
                "workflow_snapshot" | "workflow_update" => {
                    if let Ok(state) = serde_json::from_str(&text) {
                        on_event(StudioEvent::WorkflowState(Box::new(state)));
                    }
                }
                "workflow_agent" => {
                    if let Ok(state) = serde_json::from_str(&text) {
                        on_event(StudioEvent::WorkflowAgent(Box::new(state)));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

pub(crate) trait CameraApiPath {
    /// URL of this camera's live MJPEG stream.
    fn camera_stream_url(&self) -> String;
}

impl CameraApiPath for str {
    fn camera_stream_url(&self) -> String {
        format!("/api/cameras/stream.mjpeg?id={}", self.url_component())
    }
}

pub(crate) trait EpisodeApiPath {
    /// Complete visual-inspection snapshot for this persisted Spiral episode.
    fn inspection_url(&self) -> String;

    /// Browser-native MP4 endpoint for one segment of this episode's camera stream.
    fn video_url(&self, stream: &str, segment: u32) -> String;
}

impl EpisodeApiPath for str {
    fn inspection_url(&self) -> String {
        format!("/api/episodes/{}/inspection", self.url_component())
    }

    fn video_url(&self, stream: &str, segment: u32) -> String {
        format!(
            "/api/episodes/{}/videos/{}?segment={segment}",
            self.url_component(),
            stream.url_component()
        )
    }
}

/// Percent-encode a value for use in a query string.
///
/// Camera identifiers are device paths on Linux, so at minimum the separators
/// and reserved characters have to survive the round trip.
trait UrlComponent {
    fn url_component(&self) -> String;
}

impl UrlComponent for str {
    fn url_component(&self) -> String {
        let mut encoded = String::with_capacity(self.len());
        for byte in self.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => encoded.push_str(&format!("%{byte:02X}")),
            }
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_survive_the_query_string() {
        assert_eq!("/dev/video0".url_component(), "%2Fdev%2Fvideo0");
        assert_eq!(
            "/dev/video0".camera_stream_url(),
            "/api/cameras/stream.mjpeg?id=%2Fdev%2Fvideo0"
        );
        // macOS reports opaque identifiers that may contain anything at all.
        assert_eq!("cam 1&x=2".url_component(), "cam%201%26x%3D2");
        assert_eq!("plain-id_1.0~".url_component(), "plain-id_1.0~");
        assert_eq!(
            "episode 1".inspection_url(),
            "/api/episodes/episode%201/inspection"
        );
        assert_eq!(
            "episode 1".video_url("arm/camera", 2),
            "/api/episodes/episode%201/videos/arm%2Fcamera?segment=2"
        );
    }
}
