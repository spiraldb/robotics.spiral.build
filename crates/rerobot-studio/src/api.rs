//! Talking to the studio server.
//!
//! Requests go to the same origin the page was served from, so the UI works
//! unchanged whether the studio is on localhost or on the robot's own machine.

use std::collections::BTreeMap;

#[cfg(feature = "web")]
use futures_util::{StreamExt, stream};
#[cfg(feature = "web")]
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::{Method, RequestBuilder};
use js_sys::Uint8Array;
use rerobot_core::api::{ApiError, EpisodeSummary};
#[cfg(feature = "web")]
use rerobot_core::api::{StudioSnapshot, TelemetryFrame};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fetch and decode a JSON resource.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    decode(&request(Method::GET, path, None).await?, path)
}

/// Send a request whose response body does not matter.
pub async fn send<B: Serialize>(
    method: Method,
    path: &str,
    body: Option<&B>,
) -> Result<(), String> {
    let body = body.map(to_value).transpose()?;
    request(method, path, body).await.map(|_| ())
}

/// Send a request and decode its JSON response.
pub async fn send_json<B: Serialize, T: DeserializeOwned>(
    method: Method,
    path: &str,
    body: Option<&B>,
) -> Result<T, String> {
    let body = body.map(to_value).transpose()?;
    decode(&request(method, path, body).await?, path)
}

/// POST with no request body.
pub async fn post(path: &str) -> Result<(), String> {
    send::<()>(Method::POST, path, None).await
}

/// Filesystem-free metadata for one locally recorded camera stream.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct VideoAssetMetadata {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TimelineMetadata {
    pub samples: u64,
    pub first_seconds: Option<f64>,
    pub last_seconds: Option<f64>,
}

/// Local episode detail used to align timeline samples and recorded media.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EpisodeTimelineResponse {
    pub episode: EpisodeSummary,
    pub samples: TimelineMetadata,
    pub videos: Vec<VideoAssetMetadata>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TimelineArmSample {
    pub arm: String,
    pub normalized: BTreeMap<String, Option<f64>>,
    pub raw: BTreeMap<String, i64>,
    pub velocity: Option<BTreeMap<String, Option<f64>>>,
    pub effort: Option<BTreeMap<String, Option<f64>>>,
    pub action: Option<BTreeMap<String, Option<f64>>>,
}

/// The nearest stored sample to a committed historical playhead position.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TimelineSample {
    pub frame: u64,
    pub seconds: f64,
    pub follow_error_degrees: Option<f64>,
    pub arms: Vec<TimelineArmSample>,
}

/// Upload one file and decode the server's JSON response.
pub async fn upload_json<T: DeserializeOwned>(
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
        return decode(&text, path);
    }
    Err(serde_json::from_str::<ApiError>(&text)
        .map(|failure| failure.error)
        .unwrap_or_else(|_| format!("upload to {path} failed with status {status}")))
}

fn to_value<B: Serialize>(body: &B) -> Result<Value, String> {
    serde_json::to_value(body).map_err(|error| format!("could not encode the request: {error}"))
}

fn decode<T: DeserializeOwned>(body: &str, path: &str) -> Result<T, String> {
    serde_json::from_str(body)
        .map_err(|error| format!("could not read the response from {path}: {error}"))
}

async fn request(method: Method, path: &str, body: Option<Value>) -> Result<String, String> {
    let builder = RequestBuilder::new(path).method(method);
    let request = match body {
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
        .map(|failure| failure.error)
        .unwrap_or_else(|_| format!("request to {path} failed with status {status}")))
}

/// One message from the studio's event stream.
#[cfg(feature = "web")]
pub enum StudioEvent {
    Snapshot(Box<StudioSnapshot>),
    Telemetry(Box<TelemetryFrame>),
}

/// Consume the server's event stream, handing every message to `on_event`.
///
/// Returns when the connection closes, which the caller treats as a signal to
/// reconnect. The [`EventSource`] is owned by this future, so dropping the
/// future closes the connection.
#[cfg(feature = "web")]
pub async fn stream_events<F>(mut on_event: F) -> Result<(), String>
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

    let mut events = stream::select(snapshots, telemetry);
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
            _ => {}
        }
    }
    Ok(())
}

/// URL of a camera's live MJPEG stream.
pub fn camera_stream_url(id: &str) -> String {
    format!("/api/cameras/stream.mjpeg?id={}", encode_query(id))
}

/// Timeline detail endpoint for one local episode.
pub fn episode_timeline_url(episode: &str) -> String {
    format!("/api/episodes/{}/timeline", encode_query(episode))
}

/// Random-access sample endpoint for one local episode.
pub fn timeline_sample_url(episode: &str, seconds: f64) -> String {
    format!(
        "/api/episodes/{}/timeline/sample?at={seconds:.6}",
        encode_query(episode)
    )
}

/// Browser-native MP4 endpoint for one segment of a local camera stream.
pub fn episode_video_url(episode: &str, stream: &str, segment: u32) -> String {
    format!(
        "/api/episodes/{}/videos/{}?segment={segment}",
        encode_query(episode),
        encode_query(stream)
    )
}

/// URL of one MP4 stored in Spiral for browser-native preview.
pub fn spiral_video_url(episode: &str, stream: &str) -> String {
    format!(
        "/api/spiral/video.mp4?episode={}&stream={}",
        encode_query(episode),
        encode_query(stream)
    )
}

/// Endpoint for publishing or retrying one completed local recovery copy.
pub fn publication_url(episode: &str) -> String {
    format!("/api/publications/{}", encode_query(episode))
}

/// Percent-encode a value for use in a query string.
///
/// Camera identifiers are device paths on Linux, so at minimum the separators
/// and reserved characters have to survive the round trip.
fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_survive_the_query_string() {
        assert_eq!(encode_query("/dev/video0"), "%2Fdev%2Fvideo0");
        assert_eq!(
            camera_stream_url("/dev/video0"),
            "/api/cameras/stream.mjpeg?id=%2Fdev%2Fvideo0"
        );
        // macOS reports opaque identifiers that may contain anything at all.
        assert_eq!(encode_query("cam 1&x=2"), "cam%201%26x%3D2");
        assert_eq!(encode_query("plain-id_1.0~"), "plain-id_1.0~");
        assert_eq!(
            spiral_video_url("episode 1", "arm/camera"),
            "/api/spiral/video.mp4?episode=episode%201&stream=arm%2Fcamera"
        );
        assert_eq!(
            episode_timeline_url("episode 1"),
            "/api/episodes/episode%201/timeline"
        );
        assert_eq!(
            timeline_sample_url("episode 1", 1.25),
            "/api/episodes/episode%201/timeline/sample?at=1.250000"
        );
        assert_eq!(
            episode_video_url("episode 1", "arm/camera", 2),
            "/api/episodes/episode%201/videos/arm%2Fcamera?segment=2"
        );
    }
}
