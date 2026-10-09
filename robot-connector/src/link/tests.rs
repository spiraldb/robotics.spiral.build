//! The link against a scripted Studio.

use std::collections::VecDeque;
use std::sync::Mutex;

use spiral_robotics_common::robot_connector::{
    LINK_ANSWERS_PATH, LINK_TELEMETRY_PATH, STATUS_HEADER,
};
use spiral_robotics_hardware::NoSerialDriver;
use tokio::sync::{mpsc, watch};

use super::*;
use crate::{DeviceDrivers, EncoderDriver, NoCameraDriver, RobotConnector};

/// Answers each poll from a script, then holds the next one open as an idle
/// long poll would; reports every request the connector sends.
struct ScriptedStudio {
    polls: Mutex<VecDeque<Response<Vec<u8>>>>,
    sent: mpsc::UnboundedSender<Request<Vec<u8>>>,
}

impl LinkDriver for ScriptedStudio {
    fn send(&self, request: Request<Vec<u8>>) -> BoxFuture<'static, io::Result<Response<Vec<u8>>>> {
        let polled = request.uri().path() == LINK_NEXT_PATH;
        let scripted = polled.then(|| self.polls.lock().unwrap().pop_front());
        let _ = self.sent.send(request);
        Box::pin(async move {
            match scripted {
                None => Ok(Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(Vec::new())
                    .unwrap()),
                Some(Some(answer)) => Ok(answer),
                Some(None) => std::future::pending().await,
            }
        })
    }
}

struct NoEncoder;

impl EncoderDriver for NoEncoder {
    fn encode(
        &self,
        _fps: u32,
        _frames: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        Box::pin(async { Err("this connector films nothing".to_owned()) })
    }
}

#[test]
fn a_studio_link_needs_a_web_address_and_a_token() {
    assert!(StudioLink::new("https://robotics.spiraldb.com/", "rc_token").is_ok());
    assert!(StudioLink::new("robotics.spiraldb.com", "rc_token").is_err());
    assert!(StudioLink::new("http://localhost:3000", " ").is_err());
}

#[tokio::test]
async fn a_linked_connector_answers_relayed_requests_and_posts_what_studio_watches() {
    let relayed = Response::builder()
        .status(StatusCode::OK)
        .header(
            REQUEST_HEADER,
            serde_json::to_string(&RelayedRequestHead::new(7, "GET", "/api/robots")).unwrap(),
        )
        .header(WATCH_HEADER, r#"["telemetry"]"#)
        .body(Vec::new())
        .unwrap();
    let (sent, mut studio) = mpsc::unbounded_channel();
    let link = ScriptedStudio {
        polls: Mutex::new(VecDeque::from([relayed])),
        sent,
    };
    let data = tempfile::tempdir().unwrap();
    let connector = RobotConnector::open(
        DeviceDrivers::new(
            Arc::new(NoSerialDriver),
            Arc::new(NoCameraDriver),
            Arc::new(NoEncoder),
            Arc::new(link),
        ),
        data.path().to_owned(),
    )
    .unwrap();
    let (stop, shutdown) = watch::channel(false);
    let running = tokio::spawn(connector.run(
        StudioLink::new("http://studio.test", "rc_token").unwrap(),
        shutdown,
    ));

    let poll = studio.recv().await.unwrap();
    assert_eq!(
        poll.uri().to_string(),
        "http://studio.test/api/robot-connector/link/next"
    );
    assert_eq!(poll.headers()[header::AUTHORIZATION], "Bearer rc_token");
    let health: RobotConnectorHealth =
        serde_json::from_slice(poll.headers()[HEALTH_HEADER].as_bytes()).unwrap();
    assert_eq!(health.version(), env!("CARGO_PKG_VERSION"));

    let (mut answered, mut published) = (false, false);
    while !(answered && published) {
        let request = tokio::time::timeout(Duration::from_secs(5), studio.recv())
            .await
            .expect("the connector posts in time")
            .unwrap();
        let path = request.uri().path().to_owned();
        if path == format!("{LINK_ANSWERS_PATH}/7") {
            assert_eq!(request.headers()[STATUS_HEADER], "200");
            assert_eq!(request.headers()[header::CONTENT_TYPE], "application/json");
            assert_eq!(request.body(), b"[]");
            answered = true;
        } else if path == LINK_TELEMETRY_PATH {
            let telemetry: serde_json::Value = serde_json::from_slice(request.body()).unwrap();
            assert!(telemetry.get("loop_hz").is_some(), "{telemetry}");
            published = true;
        }
    }

    stop.send(true).unwrap();
    running.await.unwrap().unwrap();
}
