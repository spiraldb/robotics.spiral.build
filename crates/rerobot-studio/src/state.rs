//! Studio state shared by every screen.

use std::future::Future;
use std::rc::Rc;

use dioxus::prelude::*;
use rerobot_core::api::{
    EpisodeSummary, ModeSelection, RecordingProgress, ReplayProgress, RobotSummary, RobotTelemetry,
    StudioSnapshot, TelemetryFrame,
};
use rerobot_core::urdf::{SO101_URDF, Urdf, UrdfError};

#[cfg(feature = "web")]
use crate::api::{self, StudioEvent};

/// The robot description the studio poses, draws, and measures from.
///
/// Parsed once for the page and shared, so the picture in the viewport and the
/// numbers beside it can never come from two different readings of the file.
/// A failure is carried rather than unwrapped: the description ships with the
/// build, so a bad one is worth showing plainly instead of blanking the page.
pub type Description = Rc<Result<Urdf, UrdfError>>;

/// Parse the description and publish it to every panel.
pub fn use_description() -> Description {
    use_context_provider(|| Rc::new(Urdf::parse(SO101_URDF)))
}

/// Read the shared robot description from context.
pub fn description() -> Description {
    use_context::<Description>()
}

/// A short message shown to the operator after an action.
///
/// A notice carries its own expiry rather than leaving one to whoever renders
/// it: every one of these used to sit on the bench until it was dismissed by
/// hand, and "Episode saved." was still on screen forty takes later. Putting
/// the deadline in the value means a notice cannot be created without one.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub failed: bool,
    /// Wall-clock milliseconds after which this notice comes off the page.
    pub until_ms: f64,
}

impl Notice {
    pub fn new(text: impl Into<String>, failed: bool) -> Self {
        let text = text.into();
        let until_ms = now_ms() + f64::from(linger_ms(&text, failed));
        Self {
            text,
            failed,
            until_ms,
        }
    }

    /// True once `now` has passed this notice's deadline.
    ///
    /// Only the browser's clock loop asks; the server build never raises a
    /// notice, so it never has one to age out.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    fn expired(&self, now: f64) -> bool {
        now >= self.until_ms
    }
}

/// How long a notice stays up, scaled to how much there is to read.
///
/// A confirmation the operator glances at should not outlive the glance, and a
/// failure they have to act on should not vanish mid-sentence — so the floor
/// differs and the length is paid for either way.
fn linger_ms(text: &str, failed: bool) -> u32 {
    let reading = 3_500 + (text.chars().count() as u32) * 55;
    reading.clamp(if failed { 8_000 } else { 4_000 }, 14_000)
}

fn now_ms() -> f64 {
    #[cfg(feature = "web")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(feature = "web"))]
    {
        0.0
    }
}

/// The one selection shared by the hierarchy, viewport, cameras, and inspector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Robot(String),
    Joint { robot: String, joint: String },
    Camera { robot: String, role: String },
}

impl Selection {
    pub fn robot_id(&self) -> &str {
        match self {
            Selection::Robot(robot)
            | Selection::Joint { robot, .. }
            | Selection::Camera { robot, .. } => robot,
        }
    }

    pub fn matches_robot(&self, robot: &str) -> bool {
        self.robot_id() == robot
    }

    pub fn matches_joint(&self, robot: &str, joint: &str) -> bool {
        matches!(
            self,
            Selection::Joint {
                robot: selected_robot,
                joint: selected_joint,
            } if selected_robot == robot && selected_joint == joint
        )
    }

    pub fn matches_camera(&self, robot: &str, role: &str) -> bool {
        matches!(
            self,
            Selection::Camera {
                robot: selected_robot,
                role: selected_role,
            } if selected_robot == robot && selected_role == role
        )
    }
}

/// Everything the screens read, published by the server's event stream.
#[derive(Clone, Copy)]
pub struct Studio {
    pub snapshot: Signal<StudioSnapshot>,
    pub telemetry: Signal<TelemetryFrame>,
    /// True while the event stream is connected.
    pub live: Signal<bool>,
    /// Wall-clock time of the last valid event and a periodically updated clock.
    pub last_event_ms: Signal<f64>,
    pub clock_ms: Signal<f64>,
    pub notice: Signal<Option<Notice>>,
    /// The entity every view is currently showing and editing.
    pub selected: Signal<Option<Selection>>,
    /// What the pointer is over, wherever it is.
    ///
    /// Separate from `selected` because pointing is not choosing: hovering a
    /// joint row lights that motor on the bench and hovering the motor lights
    /// the row, without either disturbing what the operator actually picked.
    /// One object, seen twice.
    pub pointed: Signal<Option<Selection>>,
    /// The take the timeline is showing in detail.
    ///
    /// Deliberately not part of [`Selection`]: choosing a take must not point
    /// the joint inspector somewhere else, because judging a take and watching
    /// the arm that produced it are the same gesture.
    pub selected_episode: Signal<Option<String>>,
}

impl Studio {
    /// Create the shared state and start following the server.
    pub fn use_studio() -> Self {
        let studio = use_context_provider(|| Self {
            snapshot: Signal::new(StudioSnapshot::default()),
            telemetry: Signal::new(TelemetryFrame::default()),
            live: Signal::new(false),
            last_event_ms: Signal::new(0.0),
            // Keep the server's first render identical to the browser's first
            // render; the browser clock starts with the client tasks below.
            clock_ms: Signal::new(0.0),
            notice: Signal::new(None),
            selected: Signal::new(None),
            pointed: Signal::new(None),
            selected_episode: Signal::new(None),
        });
        #[cfg(feature = "web")]
        let mut studio = studio;

        // One long-lived task owns the connection and reconnects if the server
        // restarts, so the page recovers without a manual refresh.
        #[cfg(feature = "web")]
        use_future(move || async move {
            let Self {
                mut snapshot,
                mut telemetry,
                mut live,
                mut last_event_ms,
                ..
            } = studio;
            loop {
                let _ = api::stream_events(|event| {
                    // Opening an EventSource is not a connection signal. Only
                    // mark the UI live after the server has delivered a valid
                    // event, so controls never look usable during a failed
                    // connection attempt.
                    live.set(true);
                    last_event_ms.set(js_sys::Date::now());
                    match event {
                        StudioEvent::Snapshot(value) => snapshot.set(*value),
                        StudioEvent::Telemetry(frame) => telemetry.set(*frame),
                    }
                })
                .await;
                live.set(false);
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });

        // The clock the studio measures staleness against, and the one place a
        // notice is taken back off the page. Expiring it here rather than from
        // the component that draws it means every notice ages the same way, no
        // matter which panel raised it.
        #[cfg(feature = "web")]
        use_future(move || async move {
            let mut notice = studio.notice;
            loop {
                gloo_timers::future::TimeoutFuture::new(250).await;
                let now = js_sys::Date::now();
                studio.clock_ms.set(now);
                if notice
                    .peek()
                    .as_ref()
                    .is_some_and(|current| current.expired(now))
                {
                    notice.set(None);
                }
            }
        });

        studio
    }

    /// One robot's live readings, present only while it is connected.
    pub fn robot_telemetry(&self, id: &str) -> Option<RobotTelemetry> {
        if !self.controls_available() {
            return None;
        }
        self.telemetry
            .read()
            .robots
            .iter()
            .find(|robot| robot.id == id)
            .cloned()
    }

    /// Normal controls require both an open stream and a recent event.
    pub fn controls_available(&self) -> bool {
        let age = self.telemetry_age_seconds();
        (self.live)() && age.is_finite() && age < 2.5
    }

    pub fn telemetry_age_seconds(&self) -> f64 {
        let last = (self.last_event_ms)();
        if last <= 0.0 {
            return f64::INFINITY;
        }
        (((self.clock_ms)() - last) / 1000.0).max(0.0)
    }

    /// The arm the side panels are showing.
    ///
    /// The operator's choice while it still names a robot, and the first one
    /// otherwise. Deriving this rather than storing a corrected copy keeps a
    /// deleted robot from leaving the panels pointed at nothing, without an
    /// effect that writes the signal it reads.
    pub fn focused(&self) -> Option<RobotSummary> {
        let robots = self.snapshot.read();
        let chosen = self.selected.read();
        chosen
            .as_ref()
            .and_then(|selection| {
                robots
                    .robots
                    .iter()
                    .find(|robot| robot.id == selection.robot_id())
            })
            .or_else(|| robots.robots.first())
            .cloned()
    }

    /// The colour and position this arm has in the 3D scene.
    /// Which accent slot an arm owns.
    ///
    /// Derived from the arm's own role and name, never from its position in the
    /// directory: taking it from the list index meant that adding, renaming, or
    /// removing one arm silently repainted every other one, and "blue is the
    /// follower" is a thing an operator learns in the first minute and relies on
    /// while looking at the bench rather than the screen.
    pub fn position_of(&self, id: &str) -> usize {
        let kind = self
            .snapshot
            .read()
            .robots
            .iter()
            .find(|robot| robot.id == id)
            .and_then(|robot| robot.robot_type.clone());
        // Blue for the follower and violet for the leader, which is what the
        // alphabetical accident used to produce on a two-arm bench; the arms
        // keep the colours their operator already knows.
        match kind.as_deref() {
            Some(kind) if kind.contains("follower") => 0,
            Some(kind) if kind.contains("leader") => 1,
            // Everything that is not one end of the usual pair takes a stable
            // slot from its own name, out of the two the pair does not claim.
            _ => {
                let hash = id.bytes().fold(0usize, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(usize::from(byte))
                });
                2 + hash % 2
            }
        }
    }

    /// Every recorded take, oldest first.
    pub fn episodes(&self) -> Vec<EpisodeSummary> {
        self.snapshot.read().episodes.clone()
    }

    /// The take being captured right now.
    ///
    /// Unlike most readings this survives a telemetry gap on purpose: a
    /// recording that is still running is exactly what an operator needs to know
    /// about while the link is unreliable.
    pub fn recording(&self) -> Option<RecordingProgress> {
        self.telemetry.read().recording.clone()
    }

    /// The episode being played back right now.
    pub fn replay(&self) -> Option<ReplayProgress> {
        self.telemetry.read().replay.clone()
    }

    /// The take the timeline is showing, when it still exists.
    pub fn focused_episode(&self) -> Option<EpisodeSummary> {
        let episodes = self.snapshot.read();
        let chosen = self.selected_episode.read();
        chosen
            .as_ref()
            .and_then(|id| episodes.episodes.iter().find(|episode| &episode.id == id))
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

    /// Whether the configured mode is actually running.
    pub fn mode_active(&self) -> bool {
        self.snapshot.read().mode.active
    }

    /// True when `id` is the robot the operator is driving by hand.
    pub fn is_manual_target(&self, id: &str) -> bool {
        self.mode_active() && matches!(self.mode(), ModeSelection::Manual { robot } if robot == id)
    }

    /// The part this robot plays in a running teleoperation, if any.
    ///
    /// Several panels change what they say depending on whether an arm is
    /// being moved by hand or is copying one that is, and all of them should
    /// agree about which is which.
    pub fn teleoperation_role(&self, id: &str) -> Option<&'static str> {
        if !self.mode_active() {
            return None;
        }
        match self.mode() {
            ModeSelection::Teleoperate { leader, .. } if leader == id => Some("leader"),
            ModeSelection::Teleoperate { follower, .. } if follower == id => Some("follower"),
            _ => None,
        }
    }

    /// Run a request, reporting the outcome to the operator.
    ///
    /// An empty `success` message means the change is visible on screen and
    /// does not need to be announced.
    pub fn run<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        if !self.controls_available() {
            self.report("Controls are paused because telemetry is unavailable or delayed. No command was sent.");
            return;
        }
        self.run_inner(success, future);
    }

    /// Send a command whose only purpose is to move the system toward idle.
    /// This remains available during an event-stream outage because the HTTP
    /// command path may still be healthy while telemetry reconnects.
    pub fn run_safety<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }

    /// Run a file or workspace operation that does not command live hardware.
    /// These remain useful while telemetry is reconnecting.
    pub fn run_utility<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }

    fn run_inner<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let mut notice = self.notice;
        let success = success.into();
        spawn(async move {
            match future.await {
                Ok(()) if success.is_empty() => {}
                Ok(()) => notice.set(Some(Notice::new(success, false))),
                Err(error) => notice.set(Some(Notice::new(error, true))),
            }
        });
    }

    /// Report a problem the UI detected on its own.
    pub fn report(&self, message: impl Into<String>) {
        let mut notice = self.notice;
        notice.set(Some(Notice::new(message, true)));
    }
}

/// Read the shared studio state from context.
pub fn studio() -> Studio {
    use_context::<Studio>()
}

#[cfg(test)]
mod tests {
    use super::{Notice, Selection, linger_ms};

    #[test]
    fn a_notice_stays_up_long_enough_to_read_and_no_longer() {
        let glance = linger_ms("Episode saved.", false);
        let sentence = linger_ms(
            "Recording cancelled. The episode remains on disk but will not be exported.",
            false,
        );

        assert!((4_000..=5_000).contains(&glance), "{glance}");
        assert!(sentence > glance, "longer messages get longer to read");
        // A failure asks for a decision, so it gets a floor of its own even
        // when it is short.
        assert!(linger_ms("Failed.", true) >= 8_000);
        // And nothing sits on the bench forever, which is what having no
        // deadline at all amounted to.
        assert_eq!(linger_ms(&"x".repeat(400), false), 14_000);
    }

    #[test]
    fn a_notice_comes_off_the_page_on_its_own() {
        let notice = Notice::new("Episode saved.", false);

        assert!(!notice.expired(0.0), "it is up the moment it is raised");
        assert!(!notice.expired(notice.until_ms - 1.0));
        assert!(notice.expired(notice.until_ms));
    }

    #[test]
    fn canonical_selection_keeps_robot_context() {
        let joint = Selection::Joint {
            robot: "arm-a".into(),
            joint: "elbow".into(),
        };
        let camera = Selection::Camera {
            robot: "arm-a".into(),
            role: "wrist".into(),
        };

        assert_eq!(joint.robot_id(), "arm-a");
        assert!(joint.matches_joint("arm-a", "elbow"));
        assert!(camera.matches_camera("arm-a", "wrist"));
        assert!(!camera.matches_robot("arm-b"));
    }
}
