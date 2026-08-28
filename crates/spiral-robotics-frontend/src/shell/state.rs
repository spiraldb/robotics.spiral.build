//! Application state shared by every Studio feature.

use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_api::recording::{RecordingProgress, ReplayProgress};
use spiral_robotics_api::relations::RelationCatalog;
use spiral_robotics_api::robots::{ModeSelection, RobotSummary, RobotTelemetry, TelemetryFrame};
use spiral_robotics_api::rollout::RolloutSnapshot;
use spiral_robotics_api::status::StudioSnapshot;
use spiral_robotics_api::training::TrainingSnapshot;

use crate::activities::workflow::state::WorkflowController;
use crate::client::http as api;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use crate::client::http::StudioEvent;
use crate::panels::timeline::signals::SignalStore;
use crate::shell::notifications::{
    MessageAction, MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};

/// The one selection shared by the hierarchy, viewport, cameras, and inspector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    Robot(String),
    Joint { robot: String, joint: String },
    Camera { robot: String, role: String },
}

impl Selection {
    pub(crate) fn robot_id(&self) -> &str {
        match self {
            Selection::Robot(robot)
            | Selection::Joint { robot, .. }
            | Selection::Camera { robot, .. } => robot,
        }
    }

    pub(crate) fn matches_robot(&self, robot: &str) -> bool {
        self.robot_id() == robot
    }

    pub(crate) fn matches_joint(&self, robot: &str, joint: &str) -> bool {
        matches!(
            self,
            Selection::Joint {
                robot: selected_robot,
                joint: selected_joint,
            } if selected_robot == robot && selected_joint == joint
        )
    }
}

/// Choose the first configured motor only when selection does not already name
/// a valid object on an available arm. Camera selection remains intentional;
/// selecting an arm itself, arriving with no selection, or losing a stale
/// motor selection opens that arm's first motor in the inspector.
trait RobotSelection {
    fn default_joint_selection(&self, selected: Option<&Selection>) -> Option<Selection>;
}

impl RobotSelection for [RobotSummary] {
    fn default_joint_selection(&self, selected: Option<&Selection>) -> Option<Selection> {
        let robot = selected
            .and_then(|selection| self.iter().find(|robot| robot.id() == selection.robot_id()))
            .or_else(|| self.first())?;

        match selected {
            Some(Selection::Camera {
                robot: selected, ..
            }) if selected == robot.id() => return None,
            Some(Selection::Joint {
                robot: selected,
                joint,
            }) if selected == robot.id()
                && robot.motors().iter().any(|motor| motor.name() == joint) =>
            {
                return None;
            }
            _ => {}
        }

        robot.motors().first().map(|motor| Selection::Joint {
            robot: robot.id().to_owned(),
            joint: motor.name().to_owned(),
        })
    }
}

/// Shared frontend state. Live state follows the server event stream; Data
/// activity state is owned by its controller rather than this shell state.
#[derive(Clone, Copy)]
pub(crate) struct Studio {
    snapshot: Signal<StudioSnapshot>,
    telemetry: Signal<TelemetryFrame>,
    backend: BackendSignals,
    data: StudioDataSignals,
    relation_catalog: Resource<Result<RelationCatalog, String>>,
    notifications: Signal<NotificationCenterState>,
    interaction: StudioInteractionSignals,
}

#[derive(Clone, Copy)]
pub(crate) struct BackendSignals {
    /// Backend-owned lifecycle and metric history for remote policy training.
    training: Signal<TrainingSnapshot>,
    /// Backend-owned remote inference and motor-control lifecycle.
    rollout: Signal<RolloutSnapshot>,
    /// True while the event stream is connected.
    live: Signal<bool>,
    /// Wall-clock time of the last valid event and a periodically updated clock.
    last_event_ms: Signal<f64>,
    clock_ms: Signal<f64>,
}

#[derive(Clone, Copy)]
pub(crate) struct StudioDataSignals {
    /// Dataset row used by capture, import, Data filtering, and new Training
    /// runs. This is distinct from the configured Spiral namespace.
    selected_dataset: Signal<String>,
    /// Joint angles for the take being recorded, or the last one recorded.
    ///
    /// Filled from the event stream rather than from the panel that draws it:
    /// a take captured with the Signals region collapsed is still a take, and
    /// arriving to find it empty would be the interface deciding what was
    /// measured. See [`crate::panels::timeline::signals`].
    recording_signals: Signal<SignalStore>,
}

#[derive(Clone, Copy)]
struct StudioInteractionSignals {
    selected: Signal<Option<Selection>>,
    pointed: Signal<Option<Selection>>,
}

impl Studio {
    fn now_ms() -> f64 {
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        {
            js_sys::Date::now()
        }
        #[cfg(not(all(feature = "web", target_arch = "wasm32")))]
        {
            0.0
        }
    }

    pub(crate) const fn snapshot(self) -> Signal<StudioSnapshot> {
        self.snapshot
    }

    pub(crate) const fn telemetry(self) -> Signal<TelemetryFrame> {
        self.telemetry
    }

    pub(crate) const fn training(self) -> Signal<TrainingSnapshot> {
        self.backend.training
    }

    pub(crate) const fn rollout(self) -> Signal<RolloutSnapshot> {
        self.backend.rollout
    }

    pub(crate) const fn live(self) -> Signal<bool> {
        self.backend.live
    }

    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    const fn last_event_ms(self) -> Signal<f64> {
        self.backend.last_event_ms
    }

    pub(crate) const fn clock_ms(self) -> Signal<f64> {
        self.backend.clock_ms
    }

    pub(crate) const fn notifications(self) -> Signal<NotificationCenterState> {
        self.notifications
    }

    pub(crate) const fn selection(self) -> Signal<Option<Selection>> {
        self.interaction.selected
    }

    pub(crate) const fn pointed_selection(self) -> Signal<Option<Selection>> {
        self.interaction.pointed
    }

    pub(crate) const fn selected_dataset(self) -> Signal<String> {
        self.data.selected_dataset
    }

    pub(crate) const fn recording_signals(self) -> Signal<SignalStore> {
        self.data.recording_signals
    }

    pub(crate) fn relation_catalog_result(&self) -> Option<Result<RelationCatalog, String>> {
        self.relation_catalog.read_unchecked().to_owned()
    }

    pub(crate) fn relation_catalog(&self) -> Option<RelationCatalog> {
        self.relation_catalog_result().and_then(Result::ok)
    }

    pub(crate) fn relation_catalog_error(&self) -> Option<String> {
        self.relation_catalog_result().and_then(Result::err)
    }

    pub(crate) fn refresh_relation_catalog(mut self) {
        self.relation_catalog.restart();
    }

    /// Create the shared state and its project-scoped catalog resource.
    pub(crate) fn use_studio(query_available: bool) -> Self {
        // Hooks own the lifetime of every signal. The context publishes only
        // their handles; it does not construct unscoped reactive state inside
        // a provider closure.
        let snapshot = use_signal(StudioSnapshot::default);
        let telemetry = use_signal(TelemetryFrame::default);
        let training = use_signal(TrainingSnapshot::default);
        let rollout = use_signal(RolloutSnapshot::default);
        let live = use_signal(|| false);
        let last_event_ms = use_signal(|| 0.0);
        let clock_ms = use_signal(|| 0.0);
        let notifications = use_signal(NotificationCenterState::default);
        let selected = use_signal(|| None);
        let pointed = use_signal(|| None);
        let selected_dataset = use_signal(|| "default".to_owned());
        let recording_signals = use_signal(SignalStore::default);
        let relation_catalog = use_resource(use_reactive(
            (&query_available,),
            move |(query_available,)| async move {
                if query_available {
                    api::Http::get::<RelationCatalog>("/api/spiral/relations").await
                } else {
                    Ok(RelationCatalog::default())
                }
            },
        ));
        let studio = use_context_provider(|| Self {
            snapshot,
            telemetry,
            backend: BackendSignals {
                training,
                rollout,
                live,
                last_event_ms,
                // Keep the server's first render identical to the browser's first
                // render; the browser clock starts with the client tasks below.
                clock_ms,
            },
            data: StudioDataSignals {
                selected_dataset,
                recording_signals,
            },
            relation_catalog,
            notifications,
            interaction: StudioInteractionSignals { selected, pointed },
        });
        let mut default_selection = studio.selection();
        use_effect(move || {
            let next = {
                let snapshot_signal = studio.snapshot();
                let snapshot = snapshot_signal.read();
                let selected = default_selection.read();
                snapshot.robots().default_joint_selection(selected.as_ref())
            };
            if let Some(next) = next {
                default_selection.set(Some(next));
            }
        });
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        let studio = studio;

        // The clock the studio measures staleness and message prominence
        // against. Notification history does not expire; only its compact
        // status-bar preview does.
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        use_future(move || async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(250).await;
                let now = js_sys::Date::now();
                studio.clock_ms().set(now);
            }
        });

        studio
    }

    /// Follow the one backend stream and hand feature events directly to the
    /// controller that owns their accepted state.
    pub(crate) fn use_event_stream(self, workflow: WorkflowController) {
        #[cfg(not(all(feature = "web", target_arch = "wasm32")))]
        let _ = workflow;
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        use_future(move || async move {
            let studio = self;
            let mut snapshot = studio.snapshot();
            let mut telemetry = studio.telemetry();
            let mut training = studio.training();
            let mut rollout = studio.rollout();
            let mut live = studio.live();
            let mut last_event_ms = studio.last_event_ms();
            let mut recording_signals = studio.recording_signals();
            loop {
                let _ = api::Http::stream_events(|event| {
                    live.set(true);
                    last_event_ms.set(js_sys::Date::now());
                    match event {
                        StudioEvent::Snapshot(value) => snapshot.set(*value),
                        StudioEvent::TrainingSnapshot(value) => training.set(*value),
                        StudioEvent::TrainingUpdate(update) => training.write().apply(*update),
                        StudioEvent::Rollout(value) => rollout.set(*value),
                        StudioEvent::WorkflowState(value) => workflow.accept_streamed_state(*value),
                        StudioEvent::WorkflowAgent(value) => workflow.accept_streamed_agent(*value),
                        StudioEvent::Telemetry(frame) => {
                            if frame.recording().is_some() || recording_signals.peek().live() {
                                recording_signals.write().observe(&frame);
                            }
                            telemetry.set(*frame);
                        }
                    }
                })
                .await;
                live.set(false);
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }

    /// One robot's live readings, present only while it is connected.
    pub(crate) fn robot_telemetry(&self, id: &str) -> Option<RobotTelemetry> {
        if !self.controls_available() {
            return None;
        }
        self.telemetry
            .read()
            .robots()
            .iter()
            .find(|robot| robot.id() == id)
            .cloned()
    }

    /// Normal controls require both an open stream and a recent event.
    pub(crate) fn controls_available(&self) -> bool {
        let age = self.telemetry_age_seconds();
        (self.backend.live)()
            && self.snapshot.read().readiness().catalog_available()
            && age.is_finite()
            && age < 2.5
    }

    pub(crate) fn telemetry_age_seconds(&self) -> f64 {
        let last = (self.backend.last_event_ms)();
        if last <= 0.0 {
            return f64::INFINITY;
        }
        (((self.backend.clock_ms)() - last) / 1000.0).max(0.0)
    }

    /// The arm the side panels are showing.
    ///
    /// The operator's choice while it still names a robot, and the first one
    /// otherwise. Deriving this rather than storing a corrected copy keeps a
    /// deleted robot from leaving the panels pointed at nothing, without an
    /// effect that writes the signal it reads.
    pub(crate) fn focused(&self) -> Option<RobotSummary> {
        let robots = self.snapshot.read();
        let chosen = self.interaction.selected.read();
        chosen
            .as_ref()
            .and_then(|selection| {
                robots
                    .robots()
                    .iter()
                    .find(|robot| robot.id() == selection.robot_id())
            })
            .or_else(|| robots.robots().first())
            .cloned()
    }

    /// The colour and position this arm has in the 3D scene.
    /// Which accent slot an arm owns.
    ///
    /// Derived from the arm's own role and name, never from its position in the
    /// catalog: taking it from the list index meant that adding, renaming, or
    /// removing one arm silently repainted every other one, and "blue is the
    /// follower" is a thing an operator learns in the first minute and relies on
    /// while looking at the bench rather than the screen.
    pub(crate) fn position_of(&self, id: &str) -> usize {
        let snapshot = self.snapshot.read();
        let kind = snapshot
            .robots()
            .iter()
            .find(|robot| robot.id() == id)
            .and_then(|robot| robot.robot_type().map(str::to_owned));
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

    /// The take being captured right now.
    ///
    /// Unlike most readings this survives a telemetry gap on purpose: a
    /// recording that is still running is exactly what an operator needs to know
    /// about while the link is unreliable.
    pub(crate) fn recording(&self) -> Option<RecordingProgress> {
        self.telemetry.read().recording().cloned()
    }

    /// The episode being played back right now.
    pub(crate) fn replay(&self) -> Option<ReplayProgress> {
        self.telemetry.read().replay().cloned()
    }

    /// Robots whose motor bus is currently open.
    pub(crate) fn connected_robots(&self) -> Vec<RobotSummary> {
        self.snapshot
            .read()
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .cloned()
            .collect()
    }

    /// The mode the studio is in.
    pub(crate) fn mode(&self) -> ModeSelection {
        self.snapshot.read().mode().selection().to_owned()
    }

    /// Whether the configured mode is actually running.
    pub(crate) fn mode_active(&self) -> bool {
        self.snapshot.read().mode().active()
    }

    /// True when `id` is the robot the operator is driving by hand.
    pub(crate) fn is_manual_target(&self, id: &str) -> bool {
        self.mode_active() && matches!(self.mode(), ModeSelection::Manual { robot } if robot == id)
    }

    /// Run a request, reporting the outcome to the operator.
    ///
    /// An empty `success` message means the change is visible on screen and
    /// does not need to be announced.
    pub(crate) fn run<F>(&self, success: impl Into<String>, future: F)
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
    pub(crate) fn run_safety<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }

    /// Run a data or configuration operation that does not command live hardware.
    /// These remain useful while telemetry is reconnecting.
    pub(crate) fn run_utility<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        self.run_inner(success, future);
    }

    fn run_inner<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        let studio = *self;
        let success = success.into();
        // Commands are consequences owned by the Studio session, not by the
        // button or activity that happened to launch them. Activity switches
        // unmount views, so keep accepted commands alive until the backend
        // answers and report their outcome through session-owned messages.
        dioxus::dioxus_core::spawn_forever(async move {
            match future.await {
                Ok(()) if success.is_empty() => {}
                Ok(()) => studio.report_success(success),
                Err(error) => studio.report(error),
            }
        });
    }

    /// Report a problem the UI detected on its own.
    pub(crate) fn report(&self, message: impl Into<String>) {
        self.report_from(MessageSource::Studio, message);
    }

    /// Confirm a completed browser-side utility action.
    pub(crate) fn report_success(&self, message: impl Into<String>) {
        self.report_success_from(MessageSource::Studio, message);
    }

    pub(crate) fn report_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(message, MessageSeverity::Error, source));
    }

    pub(crate) fn report_success_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(
            message,
            MessageSeverity::Success,
            source,
        ));
    }

    pub(crate) fn report_info_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(message, MessageSeverity::Info, source));
    }

    pub(crate) fn report_warning_from(&self, source: MessageSource, message: impl Into<String>) {
        self.notify(MessageNotice::new(
            message,
            MessageSeverity::Warning,
            source,
        ));
    }

    pub(crate) fn report_episode_success(
        &self,
        message: impl Into<String>,
        episode: impl Into<String>,
    ) {
        self.notify(
            MessageNotice::new(message, MessageSeverity::Success, MessageSource::Live)
                .with_action(MessageAction::OpenEpisode(episode.into())),
        );
    }

    fn notify(&self, notice: MessageNotice) {
        let mut notifications = self.notifications;
        notifications.write().push(notice, Self::now_ms());
    }
}

/// Read the shared studio state from context.
pub(crate) fn studio() -> Studio {
    use_context::<Studio>()
}

#[cfg(test)]
mod tests {
    use spiral_robotics_api::robots::MotorDirection;
    use spiral_robotics_api::robots::{
        ConnectionStatus, MotorSummary, RobotAvailability, RobotSummary,
    };

    use super::{RobotSelection, Selection};

    fn robot(id: &str, motors: &[&str]) -> RobotSummary {
        RobotSummary::new(
            id,
            None,
            RobotAvailability::new(false, None, ConnectionStatus::default()),
        )
        .with_motors(
            motors
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    MotorSummary::new(*name, index as u32 + 1, MotorDirection::Normal)
                })
                .collect(),
        )
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
        assert_eq!(camera.robot_id(), "arm-a");
        assert!(!camera.matches_robot("arm-b"));
    }

    #[test]
    fn the_first_motor_is_the_default_object_on_the_focused_arm() {
        let robots = [
            robot("follower", &["shoulder_pan", "elbow_flex"]),
            robot("leader", &["shoulder_lift"]),
        ];

        assert_eq!(
            robots.default_joint_selection(None),
            Some(Selection::Joint {
                robot: "follower".into(),
                joint: "shoulder_pan".into(),
            })
        );
        assert_eq!(
            robots.default_joint_selection(Some(&Selection::Robot("leader".into()))),
            Some(Selection::Joint {
                robot: "leader".into(),
                joint: "shoulder_lift".into(),
            })
        );
    }

    #[test]
    fn an_intentional_camera_or_valid_motor_selection_is_preserved() {
        let robots = [robot("follower", &["shoulder_pan"])];
        let camera = Selection::Camera {
            robot: "follower".into(),
            role: "top".into(),
        };
        let joint = Selection::Joint {
            robot: "follower".into(),
            joint: "shoulder_pan".into(),
        };

        assert_eq!(robots.default_joint_selection(Some(&camera)), None);
        assert_eq!(robots.default_joint_selection(Some(&joint)), None);
    }
}
