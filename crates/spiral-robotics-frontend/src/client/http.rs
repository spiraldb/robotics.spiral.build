//! Browser event streams and URLs for the intentional HTTP media/download surfaces.
//!
//! Requests go to the same origin the page was served from, so the UI works
//! unchanged whether the studio is on localhost or on the robot's own machine.

#[cfg(all(feature = "web", target_arch = "wasm32"))]
use futures_util::{StreamExt, stream};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use gloo_net::eventsource::futures::EventSource;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::robots::TelemetryFrame;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::rollout::RolloutSnapshot;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::status::StudioSnapshot;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::training::{TrainingSnapshot, TrainingUpdate};
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use spiral_robotics_api::workflow::{WorkflowAgentSnapshot, WorkflowSnapshot};

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub(crate) struct Http;

/// One message from the studio's event stream.
#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub(crate) enum StudioEvent {
    Snapshot(Box<StudioSnapshot>),
    Telemetry(Box<TelemetryFrame>),
    TrainingSnapshot(Box<TrainingSnapshot>),
    TrainingUpdate(Box<TrainingUpdate>),
    Rollout(Box<RolloutSnapshot>),
    WorkflowState(Box<WorkflowSnapshot>),
    WorkflowAgent(Box<WorkflowAgentSnapshot>),
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
    /// Browser-native MP4 endpoint for one segment of this episode's camera stream.
    fn video_url(&self, stream: &str, segment: u32) -> String;
}

impl EpisodeApiPath for str {
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
            "episode 1".video_url("arm/camera", 2),
            "/api/episodes/episode%201/videos/arm%2Fcamera?segment=2"
        );
    }
}
