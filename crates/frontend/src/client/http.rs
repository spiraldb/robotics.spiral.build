//! Ordered browser events and URLs for media/download surfaces.

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
use spiral_robotics_common::project::{ProjectEvent, ProjectSessionInfo};
use spiral_robotics_common::robots::TelemetryFrame;
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::status::StudioSnapshot;
use spiral_robotics_common::training::{TrainingSnapshot, TrainingUpdate};
use spiral_robotics_common::workflow::{WorkflowAgentSnapshot, WorkflowSnapshot};

#[cfg_attr(
    not(any(test, all(feature = "web", target_arch = "wasm32"))),
    allow(dead_code)
)]
#[derive(Clone)]
pub(crate) enum StudioEvent {
    Snapshot(Box<StudioSnapshot>),
    Telemetry(Box<TelemetryFrame>),
    TrainingSnapshot(Box<TrainingSnapshot>),
    TrainingUpdate(Box<TrainingUpdate>),
    Rollout(Box<RolloutSnapshot>),
    WorkflowState(Box<WorkflowSnapshot>),
    WorkflowAgent(Box<WorkflowAgentSnapshot>),
}

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
pub(crate) enum ProjectStreamMessage {
    Session(ProjectSessionInfo),
    Event(ProjectEvent<StudioEvent>),
    Connected(bool),
}

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
impl ProjectStreamMessage {
    pub(crate) fn decode(kind: &str, text: &str) -> Option<Self> {
        if kind == "project_session" {
            return serde_json::from_str(text).ok().map(Self::Session);
        }
        let envelope: ProjectEvent<serde_json::Value> = serde_json::from_str(text).ok()?;
        let (session_id, payload) = envelope.into_parts();
        let event = match kind {
            "state" => StudioEvent::Snapshot(Box::new(serde_json::from_value(payload).ok()?)),
            "telemetry" => StudioEvent::Telemetry(Box::new(serde_json::from_value(payload).ok()?)),
            "training_snapshot" => {
                StudioEvent::TrainingSnapshot(Box::new(serde_json::from_value(payload).ok()?))
            }
            "training_update" => {
                StudioEvent::TrainingUpdate(Box::new(serde_json::from_value(payload).ok()?))
            }
            "rollout_state" => {
                StudioEvent::Rollout(Box::new(serde_json::from_value(payload).ok()?))
            }
            "workflow_snapshot" | "workflow_update" => {
                StudioEvent::WorkflowState(Box::new(serde_json::from_value(payload).ok()?))
            }
            "workflow_agent" => {
                StudioEvent::WorkflowAgent(Box::new(serde_json::from_value(payload).ok()?))
            }
            _ => return None,
        };
        Some(Self::Event(ProjectEvent::new(session_id, event)))
    }
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub(crate) struct Http;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
impl Http {
    pub(crate) async fn stream_events(
        on_event: impl FnMut(ProjectStreamMessage) + 'static,
    ) -> Result<(), String> {
        let _connection = BrowserEventConnection::new(on_event)?;
        // EventSource reconnects itself. Dropping this scoped future closes it
        // and removes its listeners; callbacks retain native dispatch order.
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
struct BrowserEventConnection {
    source: web_sys::EventSource,
    listeners: Vec<(
        String,
        wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    )>,
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
impl BrowserEventConnection {
    fn new(on_event: impl FnMut(ProjectStreamMessage) + 'static) -> Result<Self, String> {
        use wasm_bindgen::JsCast as _;
        let source = web_sys::EventSource::new("/api/events")
            .map_err(|error| format!("could not open the event stream: {error:?}"))?;
        let on_event = std::rc::Rc::new(std::cell::RefCell::new(on_event));
        let mut connection = Self {
            source,
            listeners: Vec::new(),
        };
        for kind in [
            "open",
            "error",
            "project_session",
            "state",
            "telemetry",
            "training_snapshot",
            "training_update",
            "rollout_state",
            "workflow_snapshot",
            "workflow_update",
            "workflow_agent",
        ] {
            let callback = on_event.clone();
            let listener =
                wasm_bindgen::closure::Closure::wrap(Box::new(move |event: web_sys::Event| {
                    let message = match kind {
                        "open" => Some(ProjectStreamMessage::Connected(true)),
                        "error" => Some(ProjectStreamMessage::Connected(false)),
                        _ => event
                            .dyn_ref::<web_sys::MessageEvent>()
                            .and_then(|event| event.data().as_string())
                            .and_then(|text| ProjectStreamMessage::decode(kind, &text)),
                    };
                    if let Some(message) = message {
                        callback.borrow_mut()(message);
                    }
                })
                    as Box<dyn FnMut(web_sys::Event)>);
            connection
                .source
                .add_event_listener_with_callback(kind, listener.as_ref().unchecked_ref())
                .map_err(|error| format!("could not subscribe to {kind}: {error:?}"))?;
            connection.listeners.push((kind.to_owned(), listener));
        }
        Ok(connection)
    }
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
impl Drop for BrowserEventConnection {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast as _;
        self.source.close();
        for (kind, listener) in &self.listeners {
            let _ = self
                .source
                .remove_event_listener_with_callback(kind, listener.as_ref().unchecked_ref());
        }
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
pub(crate) trait UrlComponent {
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
    fn events_require_an_explicit_project_envelope() {
        let snapshot = StudioSnapshot::default();
        let unscoped = serde_json::to_string(&snapshot).unwrap();
        assert!(ProjectStreamMessage::decode("state", &unscoped).is_none());
        let scoped = serde_json::to_string(&ProjectEvent::new("project-a", snapshot)).unwrap();
        let Some(ProjectStreamMessage::Event(event)) =
            ProjectStreamMessage::decode("state", &scoped)
        else {
            panic!("the scoped snapshot must decode");
        };
        assert_eq!(event.session_id(), "project-a");
        let announcement =
            serde_json::to_string(&ProjectSessionInfo::new("project-b", "robotics")).unwrap();
        assert!(matches!(
            ProjectStreamMessage::decode("project_session", &announcement),
            Some(ProjectStreamMessage::Session(_))
        ));
    }

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
