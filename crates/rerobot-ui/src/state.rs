//! Studio state shared by every screen.

use std::future::Future;

use dioxus::prelude::*;
use rerobot_core::api::{
    ModeSelection, RobotSummary, RobotTelemetry, StudioSnapshot, TelemetryFrame,
};

use crate::api::{self, StudioEvent};

/// A short message shown to the operator after an action.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub failed: bool,
}

/// Everything the screens read, published by the server's event stream.
#[derive(Clone, Copy)]
pub struct Studio {
    pub snapshot: Signal<StudioSnapshot>,
    pub telemetry: Signal<TelemetryFrame>,
    /// True while the event stream is connected.
    pub live: Signal<bool>,
    pub notice: Signal<Option<Notice>>,
}

impl Studio {
    /// Create the shared state and start following the server.
    pub fn use_studio() -> Self {
        let studio = use_context_provider(|| Self {
            snapshot: Signal::new(StudioSnapshot::default()),
            telemetry: Signal::new(TelemetryFrame::default()),
            live: Signal::new(false),
            notice: Signal::new(None),
        });

        // One long-lived task owns the connection and reconnects if the server
        // restarts, so the page recovers without a manual refresh.
        use_future(move || async move {
            let Self {
                mut snapshot,
                mut telemetry,
                mut live,
                ..
            } = studio;
            loop {
                live.set(true);
                let _ = api::stream_events(|event| match event {
                    StudioEvent::Snapshot(value) => snapshot.set(*value),
                    StudioEvent::Telemetry(frame) => telemetry.set(*frame),
                })
                .await;
                live.set(false);
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });

        studio
    }

    /// One robot's live readings, present only while it is connected.
    pub fn robot_telemetry(&self, id: &str) -> Option<RobotTelemetry> {
        self.telemetry
            .read()
            .robots
            .iter()
            .find(|robot| robot.id == id)
            .cloned()
    }

    /// Robots whose motor bus is currently open.
    pub fn connected_robots(&self) -> Vec<RobotSummary> {
        self.snapshot
            .read()
            .robots
            .iter()
            .filter(|robot| robot.connection.connected)
            .cloned()
            .collect()
    }

    /// The mode the studio is in.
    pub fn mode(&self) -> ModeSelection {
        self.snapshot.read().mode.selection.clone()
    }

    /// True when `id` is the robot the operator is driving by hand.
    pub fn is_manual_target(&self, id: &str) -> bool {
        matches!(self.mode(), ModeSelection::Manual { robot } if robot == id)
    }

    /// Run a request, reporting the outcome to the operator.
    ///
    /// An empty `success` message means the change is visible on screen and
    /// does not need to be announced.
    pub fn run<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let mut notice = self.notice;
        let success = success.into();
        spawn(async move {
            match future.await {
                Ok(()) if success.is_empty() => notice.set(None),
                Ok(()) => notice.set(Some(Notice {
                    text: success,
                    failed: false,
                })),
                Err(error) => notice.set(Some(Notice {
                    text: error,
                    failed: true,
                })),
            }
        });
    }

    /// Report a problem the UI detected on its own.
    pub fn report(&self, message: impl Into<String>) {
        let mut notice = self.notice;
        notice.set(Some(Notice {
            text: message.into(),
            failed: true,
        }));
    }
}

/// Read the shared studio state from context.
pub fn studio() -> Studio {
    use_context::<Studio>()
}
