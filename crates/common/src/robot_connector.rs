//! What the robot connector says about itself, and the link it keeps to
//! Studio.
//!
//! A robot rarely has a public address or a certificate, so the connector
//! dials out over plain HTTP: it long-polls Studio for the next browser
//! request to answer and posts each answer back, and while someone watches it
//! posts telemetry and camera snapshots. Every link request presents the
//! workspace's token as `Authorization: Bearer <token>`.

use serde::{Deserialize, Serialize};

/// Where Studio serves the browser the workspace's robot connector: a request
/// to `{RELAY_PREFIX}/robots` reaches the connector's `/api/robots`.
pub const RELAY_PREFIX: &str = "/api/robot-connector";

/// The connector's long poll for the next relayed request. It carries
/// [`HEALTH_HEADER`]; every answer carries [`WATCH_HEADER`], and a request to
/// answer comes as [`REQUEST_HEADER`] plus its body.
pub const LINK_NEXT_PATH: &str = "/api/robot-connector/link/next";
/// `{LINK_ANSWERS_PATH}/{id}` takes a relayed request's answer: its status in
/// [`STATUS_HEADER`], its content type, and its body.
pub const LINK_ANSWERS_PATH: &str = "/api/robot-connector/link/answers";
/// Takes the latest telemetry sample, as `GET /api/robots/telemetry` answers it.
pub const LINK_TELEMETRY_PATH: &str = "/api/robot-connector/link/telemetry";
/// `{LINK_CAMERAS_PATH}?id=<camera>` takes that camera's latest JPEG frame.
pub const LINK_CAMERAS_PATH: &str = "/api/robot-connector/link/cameras";

/// The connector's [`RobotConnectorHealth`], as JSON, on every poll.
pub const HEALTH_HEADER: &str = "X-Robot-Connector-Health";
/// The streams someone watches now, as a JSON array of [`LiveStream`].
pub const WATCH_HEADER: &str = "X-Robot-Connector-Watch";
/// The relayed request to answer, as a JSON [`RelayedRequestHead`].
pub const REQUEST_HEADER: &str = "X-Robot-Connector-Request";
/// A relayed request's answer status.
pub const STATUS_HEADER: &str = "X-Robot-Connector-Status";

/// A reachable robot connector, the build it runs, and whether it can
/// simulate arms (a microcontroller cannot draw their cameras).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorHealth {
    version: String,
    simulates_arms: bool,
}

impl RobotConnectorHealth {
    pub fn new(version: impl Into<String>, simulates_arms: bool) -> Self {
        Self {
            version: version.into(),
            simulates_arms,
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub const fn simulates_arms(&self) -> bool {
        self.simulates_arms
    }
}

/// The secret a robot connector presents to link to one workspace. Studio
/// shows it once, when it is issued, and keeps only its hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RobotConnectorToken {
    token: String,
}

impl RobotConnectorToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

/// A live stream the connector publishes while someone watches it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveStream {
    /// The robots' telemetry, as `GET /api/robots/telemetry` answers it.
    Telemetry,
    /// One camera's JPEG frames, by the ID `GET /api/hardware/status` lists.
    Camera { id: String },
}

/// A browser request Studio relays, as the connector's poll receives it; the
/// request's body is the poll response's body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayedRequestHead {
    id: u64,
    method: String,
    /// The connector's own path and query, such as `/api/robots`.
    path: String,
}

impl RelayedRequestHead {
    pub fn new(id: u64, method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id,
            method: method.into(),
            path: path.into(),
        }
    }

    pub const fn id(&self) -> u64 {
        self.id
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_headers_carry_compact_json() {
        let watched = vec![
            LiveStream::Telemetry,
            LiveStream::Camera {
                id: "/dev/video0".to_owned(),
            },
        ];
        assert_eq!(
            serde_json::to_string(&watched).unwrap(),
            r#"["telemetry",{"camera":{"id":"/dev/video0"}}]"#
        );
        assert_eq!(
            serde_json::to_string(&RelayedRequestHead::new(7, "GET", "/api/robots")).unwrap(),
            r#"{"id":7,"method":"GET","path":"/api/robots"}"#
        );
    }
}
