//! Application state shared by every Studio feature.

use std::future::Future;
use std::rc::Rc;

use dioxus::prelude::*;
use spiral_robotics_api::recording::{
    DatasetSummary, EpisodeCatalogResponse, EpisodeSummary, RecordingProgress, ReplayProgress,
};
use spiral_robotics_api::robots::{ModeSelection, RobotSummary, RobotTelemetry, TelemetryFrame};
use spiral_robotics_api::rollout::RolloutSnapshot;
use spiral_robotics_api::status::StudioSnapshot;
use spiral_robotics_api::training::TrainingSnapshot;
use spiral_robotics_api::workflow::{WorkflowAgentStateResponse, WorkflowStateResponse};
use spiral_robotics_viewport::{SO101_URDF, Urdf, UrdfError};

use crate::client::http as api;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use crate::client::http::StudioEvent;
use crate::panels::timeline::signals::SignalStore;
use crate::shell::notifications::{
    MessageAction, MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};

/// The robot description the studio poses, draws, and measures from.
///
/// Parsed once for the page and shared, so the picture in the viewport and the
/// numbers beside it can never come from two different readings of the file.
/// A failure is carried rather than unwrapped: the description ships with the
/// build, so a bad one is worth showing plainly instead of blanking the page.
pub(crate) type Description = Rc<Result<Urdf, UrdfError>>;

/// Parse the description and publish it to every panel.
pub(crate) fn use_description() -> Description {
    use_context_provider(|| Rc::new(Urdf::parse(SO101_URDF)))
}

/// Read the shared robot description from context.
pub(crate) fn description() -> Description {
    use_context::<Description>()
}

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

#[derive(Clone, Debug, Default)]
struct EpisodeCatalogState {
    datasets: Vec<DatasetSummary>,
    episodes: Vec<EpisodeSummary>,
    issues: Vec<String>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
}

/// Shared frontend state. Live state follows the server event stream; the
/// episode catalog is an explicit HTTP resource owned by the browser session.
#[derive(Clone, Copy)]
pub(crate) struct Studio {
    snapshot: Signal<StudioSnapshot>,
    telemetry: Signal<TelemetryFrame>,
    backend: BackendSignals,
    data: StudioDataSignals,
    notifications: Signal<NotificationCenterState>,
    selected: Signal<Option<Selection>>,
    pointed: Signal<Option<Selection>>,
}

#[derive(Clone, Copy)]
pub(crate) struct BackendSignals {
    /// Backend-owned lifecycle and metric history for remote policy training.
    training: Signal<TrainingSnapshot>,
    /// Backend-owned remote inference and motor-control lifecycle.
    rollout: Signal<RolloutSnapshot>,
    /// Backend-owned lifecycle and terminal payload for relation Workflows.
    workflow_state: Signal<Option<WorkflowStateResponse>>,
    /// Backend-owned Workflow Assistant readiness and durable conversation.
    workflow_agent_state: Signal<Option<WorkflowAgentStateResponse>>,
    /// True while the event stream is connected.
    live: Signal<bool>,
    /// Wall-clock time of the last valid event and a periodically updated clock.
    last_event_ms: Signal<f64>,
    clock_ms: Signal<f64>,
}

#[derive(Clone, Copy)]
pub(crate) struct StudioDataSignals {
    /// The take the timeline is showing in detail.
    ///
    /// Deliberately not part of [`Selection`]: choosing a take must not point
    /// the joint inspector somewhere else, because judging a take and watching
    /// the arm that produced it are the same gesture.
    selected_episode: Signal<Option<String>>,
    /// Dataset row used by capture, import, Data filtering, and new Training
    /// runs. This is distinct from the configured Spiral namespace.
    selected_dataset: Signal<String>,
    episode_catalog: Signal<EpisodeCatalogState>,
    /// Joint angles for the take being recorded, or the last one recorded.
    ///
    /// Filled from the event stream rather than from the panel that draws it:
    /// a take captured with the Signals region collapsed is still a take, and
    /// arriving to find it empty would be the interface deciding what was
    /// measured. See [`crate::panels::timeline::signals`].
    recording_signals: Signal<SignalStore>,
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

    pub(crate) const fn workflow_state(self) -> Signal<Option<WorkflowStateResponse>> {
        self.backend.workflow_state
    }

    pub(crate) const fn workflow_agent_state(self) -> Signal<Option<WorkflowAgentStateResponse>> {
        self.backend.workflow_agent_state
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
        self.selected
    }

    pub(crate) const fn pointed_selection(self) -> Signal<Option<Selection>> {
        self.pointed
    }

    pub(crate) const fn selected_episode(self) -> Signal<Option<String>> {
        self.data.selected_episode
    }

    pub(crate) const fn selected_dataset(self) -> Signal<String> {
        self.data.selected_dataset
    }

    pub(crate) const fn recording_signals(self) -> Signal<SignalStore> {
        self.data.recording_signals
    }

    /// Create the shared state, load the episode catalog, and follow live state.
    pub(crate) fn use_studio() -> Self {
        // Hooks own the lifetime of every signal. The context publishes only
        // their handles; it does not construct unscoped reactive state inside
        // a provider closure.
        let snapshot = use_signal(StudioSnapshot::default);
        let telemetry = use_signal(TelemetryFrame::default);
        let training = use_signal(TrainingSnapshot::default);
        let rollout = use_signal(RolloutSnapshot::default);
        let workflow_state = use_signal(|| None);
        let workflow_agent_state = use_signal(|| None);
        let live = use_signal(|| false);
        let last_event_ms = use_signal(|| 0.0);
        let clock_ms = use_signal(|| 0.0);
        let notifications = use_signal(NotificationCenterState::default);
        let selected = use_signal(|| None);
        let pointed = use_signal(|| None);
        let selected_episode = use_signal(|| None);
        let selected_dataset = use_signal(|| "default".to_owned());
        let episode_catalog = use_signal(EpisodeCatalogState::default);
        let recording_signals = use_signal(SignalStore::default);
        let studio = use_context_provider(|| Self {
            snapshot,
            telemetry,
            backend: BackendSignals {
                training,
                rollout,
                workflow_state,
                workflow_agent_state,
                live,
                last_event_ms,
                // Keep the server's first render identical to the browser's first
                // render; the browser clock starts with the client tasks below.
                clock_ms,
            },
            data: StudioDataSignals {
                selected_episode,
                selected_dataset,
                episode_catalog,
                recording_signals,
            },
            notifications,
            selected,
            pointed,
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

        // The listener deliberately precedes catalog bootstrap. Wait for its
        // explicit readiness edge before making the browser session's one
        // initial catalog request.
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        let mut catalog_bootstrap_started = use_signal(|| false);
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        use_effect(move || {
            if studio.snapshot().read().readiness().catalog_available()
                && !catalog_bootstrap_started()
            {
                catalog_bootstrap_started.set(true);
                spawn(async move {
                    let _ = studio.refresh_episode_catalog().await;
                });
            }
        });

        // One long-lived task owns the connection and reconnects if the server
        // restarts, so the page recovers without a manual refresh.
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        use_future(move || async move {
            let mut snapshot = studio.snapshot();
            let mut telemetry = studio.telemetry();
            let mut training = studio.training();
            let mut rollout = studio.rollout();
            let mut workflow_state = studio.workflow_state();
            let mut workflow_agent_state = studio.workflow_agent_state();
            let mut live = studio.live();
            let mut last_event_ms = studio.last_event_ms();
            let mut recording_signals = studio.recording_signals();
            loop {
                let _ = api::Http::stream_events(|event| {
                    // Opening an EventSource is not a connection signal. Only
                    // mark the UI live after the server has delivered a valid
                    // event, so controls never look usable during a failed
                    // connection attempt.
                    live.set(true);
                    last_event_ms.set(js_sys::Date::now());
                    match event {
                        StudioEvent::Snapshot(value) => snapshot.set(*value),
                        StudioEvent::TrainingSnapshot(value) => training.set(*value),
                        StudioEvent::TrainingUpdate(update) => training.write().apply(*update),
                        StudioEvent::Rollout(value) => rollout.set(*value),
                        StudioEvent::WorkflowState(value) => workflow_state.set(Some(*value)),
                        StudioEvent::WorkflowAgent(value) => {
                            // The backend treats its broadcast values as
                            // wakeups and emits the latest authoritative agent
                            // snapshot on this one ordered EventSource. HTTP
                            // bootstrap/command responses are guarded in the
                            // Workflow controller; do not perform a second
                            // different-scope acceptance policy here.
                            workflow_agent_state.set(Some(*value));
                        }
                        StudioEvent::Telemetry(frame) => {
                            // Only a frame that can change the store touches it.
                            // Idle writes would mark the signal dirty and
                            // needlessly rebuild the Timeline view model.
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
        let chosen = self.selected.read();
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

    /// Every recorded take, oldest first.
    pub(crate) fn episodes(&self) -> Vec<EpisodeSummary> {
        self.data.episode_catalog.read().episodes.clone()
    }

    pub(crate) fn datasets(&self) -> Vec<DatasetSummary> {
        self.data.episode_catalog.read().datasets.clone()
    }

    pub(crate) fn episodes_loading(&self) -> bool {
        self.data.episode_catalog.read().loading
    }

    pub(crate) fn episodes_loaded(&self) -> bool {
        self.data.episode_catalog.read().loaded
    }

    pub(crate) fn episode_catalog_error(&self) -> Option<String> {
        self.data.episode_catalog.read().error.clone()
    }

    pub(crate) fn episode_catalog_issues(&self) -> Vec<String> {
        self.data.episode_catalog.read().issues.clone()
    }

    /// Replace the browser's episode catalog in one request.
    pub(crate) async fn refresh_episode_catalog(mut self) -> Result<(), String> {
        {
            let mut catalog = self.data.episode_catalog.write();
            catalog.loading = true;
            catalog.error = None;
        }
        match api::Http::get::<EpisodeCatalogResponse>("/api/episodes").await {
            Ok(response) => {
                let mut catalog = self.data.episode_catalog.write();
                let (datasets, episodes, issues) = response.into_parts();
                catalog.datasets = datasets;
                catalog.episodes = episodes;
                catalog.issues = issues;
                catalog.loading = false;
                catalog.loaded = true;
                Ok(())
            }
            Err(error) => {
                let mut catalog = self.data.episode_catalog.write();
                catalog.loading = false;
                catalog.error = Some(error.clone());
                Err(format!("Could not refresh the episode catalog: {error}"))
            }
        }
    }

    /// Apply the authoritative response from a known episode mutation.
    pub(crate) fn upsert_episode(mut self, episode: EpisodeSummary) {
        let mut catalog = self.data.episode_catalog.write();
        if let Some(existing) = catalog
            .episodes
            .iter_mut()
            .find(|existing| existing.id() == episode.id())
        {
            *existing = episode;
        } else {
            catalog.episodes.push(episode);
        }
        catalog.episodes.sort_by(|left, right| {
            left.started_ms()
                .cmp(&right.started_ms())
                .then_with(|| left.id().cmp(right.id()))
        });
    }

    pub(crate) fn remove_episode(mut self, episode: &str) {
        let mut catalog = self.data.episode_catalog.write();
        catalog.episodes.retain(|existing| existing.id() != episode);
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

    /// The take the timeline is showing, when it still exists.
    pub(crate) fn focused_episode(&self) -> Option<EpisodeSummary> {
        let episodes = self.data.episode_catalog.read();
        let chosen = self.data.selected_episode.read();
        chosen
            .as_ref()
            .and_then(|id| episodes.episodes.iter().find(|episode| episode.id() == id))
            .cloned()
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
    use spiral_robotics_api::robots::{
        ConnectionStatus, MotorSummary, RobotAvailability, RobotSummary,
    };
    use spiral_robotics_core::calibration::MotorDirection;

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
