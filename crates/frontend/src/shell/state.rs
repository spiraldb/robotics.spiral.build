//! Project state and separately owned, retained state for each Studio view.

use crate::activities::data::DatasetClient;
use crate::activities::live::RecordingClient;
use crate::activities::rollout::RolloutClient;
use crate::activities::settings::SettingsClient;
use crate::activities::training::history::TrainingHistory;
use crate::shell::StudioClient;
use crate::shell::frame::{TrainingStatus, WorkflowStatusProjection};
use std::future::Future;

use dioxus::prelude::*;
use spiral_robotics_common::recording::{RecordingProgress, ReplayProgress};
use spiral_robotics_common::relations::RelationCatalog;
use spiral_robotics_common::robots::{ModeSelection, RobotSummary, RobotTelemetry, TelemetryFrame};
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::settings::DataSettings;
use spiral_robotics_common::status::StudioSnapshot;
use spiral_robotics_common::training::TrainingSnapshot;

use crate::activities::data::state::DataController;
use crate::activities::rollout::state::RolloutController;
use crate::activities::training::state::TrainingController;
use crate::activities::workflow::state::WorkflowController;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};

use crate::panels::timeline::signals::SignalStore;
use crate::panels::timeline::state::TimelineController;
use crate::shell::activity::Activity;
use crate::shell::notifications::{
    MessageNotice, MessageSeverity, MessageSource, NotificationCenterState,
};
use crate::shell::project::ProjectSessionSignals;
use crate::shell::view_state::ViewState;

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

/// Project lifetime: connection, configuration, and notifications shared by routes.
#[derive(Clone, Copy)]
pub(crate) struct StudioProjectState {
    backend: ProjectBackendSignals,
    resources: StudioProjectResources,
    notifications: Signal<NotificationCenterState>,
}

/// A view's facade over shared project state and its own retained state.
#[derive(Clone, Copy)]
pub(crate) struct Studio {
    project: StudioProjectState,
    workspace: StudioWorkspaceState,
}

#[derive(Clone, Copy)]
struct StudioWorkspaceState {
    backend: BackendSignals,
    data: StudioDataSignals,
    controllers: StudioControllers,
    interaction: StudioInteractionSignals,
    relation_catalog: Signal<Option<Result<RelationCatalog, String>>>,
    activity: Activity,
}

#[derive(Clone, Copy)]
struct StudioViewSignals {
    backend: BackendSignals,
    data: StudioDataSignals,
    interaction: StudioInteractionSignals,
    relation_catalog: Signal<Option<Result<RelationCatalog, String>>>,
}

#[derive(Clone, Copy)]
struct ProjectBackendSignals {
    snapshot: Signal<StudioSnapshot>,
    live: Signal<bool>,
    clock_ms: Signal<f64>,
}

#[derive(Clone, Copy)]
struct FeatureStatusSignals {
    training: Signal<TrainingStatus>,
    workflow: Signal<Option<WorkflowStatusProjection>>,
}

#[derive(Clone, Copy)]
struct BackendSignals {
    telemetry: Signal<TelemetryFrame>,
    /// Compact display summaries; feature views own their fetched histories.
    features: FeatureStatusSignals,
    /// Backend-owned remote inference and motor-control lifecycle.
    rollout: Signal<RolloutSnapshot>,
    /// Receipt time of the last advancing hardware sample in this view.
    last_telemetry_ms: Signal<f64>,
}

#[derive(Clone, Copy)]
struct StudioProjectResources {
    settings: Resource<Result<DataSettings, String>>,
    loaded_settings: Signal<Option<DataSettings>>,
    configuration: Memo<StudioConfiguration>,
}

#[derive(Clone, Copy)]
struct StudioControllers {
    data: DataController,

    rollout: RolloutController,
    timeline: TimelineController,
}

#[derive(Clone, Copy)]
struct StudioDataSignals {
    /// This view's dataset choice for capture or Training. Other views retain
    /// their own choice; this is distinct from the configured Spiral namespace.
    selected_dataset: Signal<String>,
    /// Joint angles for the take being recorded, or the last one recorded.
    ///
    /// Fetched from the hardware-owned sample history even while the Timeline
    /// is collapsed, and caught up by cursor after a connection gap.
    recording_signals: Signal<SignalStore>,
}

#[derive(Clone, Copy)]
struct StudioInteractionSignals {
    selected: Signal<Option<Selection>>,
    pointed: Signal<Option<Selection>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StudioConfiguration {
    dataset: String,
    spiral_query_available: bool,
}

impl Default for StudioConfiguration {
    fn default() -> Self {
        Self {
            dataset: "spiral_robotics".to_owned(),
            spiral_query_available: false,
        }
    }
}

impl StudioConfiguration {
    pub(crate) fn dataset(&self) -> &str {
        &self.dataset
    }

    pub(crate) const fn spiral_query_available(&self) -> bool {
        self.spiral_query_available
    }
}

impl BackendSignals {
    fn new() -> Self {
        Self {
            telemetry: Signal::new(TelemetryFrame::default()),
            features: FeatureStatusSignals {
                training: Signal::new(TrainingStatus::default()),
                workflow: Signal::new(None),
            },
            rollout: Signal::new(RolloutSnapshot::default()),
            last_telemetry_ms: Signal::new(0.0),
        }
    }
}

impl StudioProjectResources {
    fn use_resources() -> Self {
        let settings = QueryPolicy::new(QueryDomain::Settings, 10_000).use_resource(|| async {
            SettingsClient::current()
                .data_settings()
                .await
                .map_err(|error| error.to_string())
        });
        let mut loaded_settings = use_signal(|| None::<DataSettings>);
        use_effect(move || {
            let result = settings.read();
            if let Some(Ok(value)) = result.as_ref()
                && loaded_settings.peek().as_ref() != Some(value)
            {
                loaded_settings.set(Some(value.clone()));
            }
        });
        let loaded_configuration = loaded_settings.read().to_owned();
        let configuration = use_memo(use_reactive((&loaded_configuration,), move |(settings,)| {
            StudioConfiguration {
                dataset: settings
                    .as_ref()
                    .map(|settings| settings.spiral().namespace().to_owned())
                    .filter(|dataset| !dataset.trim().is_empty())
                    .unwrap_or_else(|| "spiral_robotics".to_owned()),
                spiral_query_available: settings
                    .as_ref()
                    .is_some_and(|settings| !settings.spiral().project_id().trim().is_empty()),
            }
        }));
        Self {
            settings,
            loaded_settings,
            configuration,
        }
    }

    fn current_configuration(&self) -> StudioConfiguration {
        (self.configuration)()
    }
}

impl StudioDataSignals {
    fn new() -> Self {
        Self {
            selected_dataset: Signal::new("default".to_owned()),
            recording_signals: Signal::new(SignalStore::default()),
        }
    }
}

impl StudioInteractionSignals {
    fn new() -> Self {
        Self {
            selected: Signal::new(None),
            pointed: Signal::new(None),
        }
    }
}

impl StudioControllers {
    fn use_controllers(project: StudioProjectResources, data: StudioDataSignals) -> Self {
        let configuration = project.current_configuration();
        let selected_dataset = data.selected_dataset.read().to_owned();
        Self {
            data: DataController::use_controller(&configuration.dataset, &selected_dataset),

            rollout: RolloutController::use_controller(),
            timeline: TimelineController::use_controller(),
        }
    }
}

impl Studio {
    fn now_ms() -> f64 {
        js_sys::Date::now()
    }

    pub(crate) fn snapshot(self) -> ReadSignal<StudioSnapshot> {
        self.project.backend.snapshot.into()
    }

    pub(crate) fn telemetry(self) -> ReadSignal<TelemetryFrame> {
        self.workspace.backend.telemetry.into()
    }

    pub(crate) fn training(self) -> ReadSignal<TrainingSnapshot> {
        TrainingHistory::current().snapshot()
    }

    pub(crate) fn rollout(self) -> ReadSignal<RolloutSnapshot> {
        self.workspace.backend.rollout.into()
    }

    pub(crate) fn live(self) -> ReadSignal<bool> {
        self.project.backend.live.into()
    }

    pub(crate) const fn clock_ms(self) -> Signal<f64> {
        self.project.backend.clock_ms
    }

    pub(crate) const fn notifications(self) -> Signal<NotificationCenterState> {
        self.project.notifications
    }

    pub(crate) const fn selection(self) -> Signal<Option<Selection>> {
        self.workspace.interaction.selected
    }

    pub(crate) const fn pointed_selection(self) -> Signal<Option<Selection>> {
        self.workspace.interaction.pointed
    }

    pub(crate) const fn selected_dataset(self) -> Signal<String> {
        self.workspace.data.selected_dataset
    }

    pub(crate) fn recording_signals(self) -> ReadSignal<SignalStore> {
        self.workspace.data.recording_signals.into()
    }

    pub(crate) fn configuration(&self) -> StudioConfiguration {
        self.project.resources.current_configuration()
    }

    pub(crate) fn loaded_settings(&self) -> Option<DataSettings> {
        self.project.resources.loaded_settings.read().to_owned()
    }

    pub(crate) fn settings_result(&self) -> Option<Result<DataSettings, String>> {
        self.project.resources.settings.read_unchecked().to_owned()
    }

    pub(crate) fn refresh_settings(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Settings]);
    }

    pub(crate) fn relation_catalog_result(&self) -> Option<Result<RelationCatalog, String>> {
        self.workspace.relation_catalog.read_unchecked().to_owned()
    }

    pub(crate) fn relation_catalog(&self) -> Option<RelationCatalog> {
        self.relation_catalog_result().and_then(Result::ok)
    }

    pub(crate) fn refresh_relation_catalog(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Catalog]);
    }

    pub(crate) const fn data_controller(self) -> DataController {
        self.workspace.controllers.data
    }

    pub(crate) fn workflow_controller(self) -> WorkflowController {
        consume_context()
    }

    pub(crate) fn training_controller(self) -> TrainingController {
        consume_context()
    }

    pub(crate) fn training_status(self) -> Signal<TrainingStatus> {
        self.workspace.backend.features.training
    }

    pub(crate) fn workflow_status(self) -> Signal<Option<WorkflowStatusProjection>> {
        self.workspace.backend.features.workflow
    }

    pub(crate) fn use_workflow_controller(self) -> WorkflowController {
        let controller = WorkflowController::use_controller(self.workspace.relation_catalog);
        use_context_provider(|| controller)
    }

    pub(crate) const fn rollout_controller(self) -> RolloutController {
        self.workspace.controllers.rollout
    }

    pub(crate) const fn timeline_controller(self) -> TimelineController {
        self.workspace.controllers.timeline
    }

    /// Each routed view retrieves its retained values and owns its mounted resources.
    pub(crate) fn use_studio(activity: Activity) -> Self {
        ViewState::use_provider(activity);
        let project = consume_context::<StudioProjectState>();
        let signals = ViewState::use_retained(|| StudioViewSignals {
            backend: BackendSignals::new(),
            data: StudioDataSignals::new(),
            interaction: StudioInteractionSignals::new(),
            relation_catalog: Signal::new(None),
        });
        let StudioViewSignals {
            backend,
            data,
            interaction,
            relation_catalog,
        } = signals;
        let controllers = StudioControllers::use_controllers(project.resources, data);
        let studio = use_context_provider(|| Self {
            project,
            workspace: StudioWorkspaceState {
                backend,
                data,
                controllers,
                interaction,
                relation_catalog,
                activity,
            },
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
        use_drop(move || studio.timeline_controller().stop_preview());
        studio
    }

    pub(crate) fn use_relation_catalog(self) {
        let mut relation_catalog = self.workspace.relation_catalog;
        let sessions = ProjectSessionSignals::current();
        let catalog_request = QueryPolicy::new(QueryDomain::Catalog, 10_000).use_when(
            move || sessions.catalog_available(),
            || async {
                DatasetClient::current()
                    .relation_catalog()
                    .await
                    .map_err(|error| error.to_string())
            },
        );
        use_effect(move || {
            let result = catalog_request.read();
            if catalog_request.finished()
                && let Some(value) = result.as_ref()
                && relation_catalog.peek().as_ref() != Some(value)
            {
                relation_catalog.set(Some(value.clone()));
            }
        });
    }

    /// Each fetched value has one writer. UI components receive read-only
    /// handles; commands invalidate these resources through ProjectClient.
    pub(crate) fn use_hardware_polling(mut self) {
        // A newly mounted view must receive a sample before enabling controls.
        use_hook(move || self.workspace.backend.last_telemetry_ms.set(0.0));
        use_drop(move || {
            self.workspace.backend.last_telemetry_ms.set(0.0);
        });
        let telemetry = QueryPolicy::new(QueryDomain::Telemetry, 50)
            .use_resource(|| StudioClient::current().telemetry_snapshot());
        use_effect(move || {
            let result = telemetry.read();
            if telemetry.finished()
                && let Some(Ok(frame)) = result.as_ref()
                && self.workspace.backend.telemetry.peek().sequence() != frame.sequence()
            {
                self.workspace.backend.last_telemetry_ms.set(Self::now_ms());
                self.workspace.backend.telemetry.set(frame.clone());
            }
        });
    }

    pub(crate) fn use_recording_polling(mut self) {
        let recording_present = use_memo(move || self.recording().is_some());
        let recording = QueryPolicy::new(QueryDomain::Recording, 200).use_when(
            move || *recording_present.read(),
            move || {
                let cursor = self.workspace.data.recording_signals.peek().cursor();
                RecordingClient::current().recording_window(cursor)
            },
        );
        use_effect(move || {
            let result = recording.read();
            if recording.finished()
                && let Some(Ok(window)) = result.as_ref()
                && window.changes(&self.workspace.data.recording_signals.peek())
            {
                self.workspace
                    .data
                    .recording_signals
                    .write()
                    .apply_window(window.clone());
            }
        });
    }

    pub(crate) fn use_rollout_polling(mut self) {
        let rollout_active =
            use_memo(move || self.workspace.backend.rollout.read().phase().active());
        let rollout = QueryPolicy::new(QueryDomain::Rollout, 500)
            .with_activity(rollout_active.into())
            .use_resource(|| RolloutClient::current().rollout_snapshot());
        use_effect(move || {
            let result = rollout.read();
            if rollout.finished()
                && let Some(Ok(value)) = result.as_ref()
                && *self.workspace.backend.rollout.peek() != *value
            {
                self.workspace.backend.rollout.set(value.clone());
            }
        });
    }

    /// One robot's live readings, present only while it is connected.
    pub(crate) fn robot_telemetry(&self, id: &str) -> Option<RobotTelemetry> {
        if !self.controls_available() {
            return None;
        }
        self.workspace
            .backend
            .telemetry
            .read()
            .robots()
            .iter()
            .find(|robot| robot.id() == id)
            .cloned()
    }

    /// Normal controls require current project readiness and fresh hardware samples.
    pub(crate) fn controls_available(&self) -> bool {
        let active = self.workspace.activity;
        let age = self.telemetry_age_seconds();
        active.uses_hardware()
            && (self.project.backend.live)()
            && ProjectSessionSignals::current().catalog_available()
            && age.is_finite()
            && age < 2.5
    }

    pub(crate) fn telemetry_age_seconds(&self) -> f64 {
        let last = (self.workspace.backend.last_telemetry_ms)();
        if last <= 0.0 {
            return f64::INFINITY;
        }
        (((self.project.backend.clock_ms)() - last) / 1000.0).max(0.0)
    }

    /// The arm the side panels are showing.
    ///
    /// The operator's choice while it still names a robot, and the first one
    /// otherwise. Deriving this rather than storing a corrected copy keeps a
    /// deleted robot from leaving the panels pointed at nothing, without an
    /// effect that writes the signal it reads.
    pub(crate) fn focused(&self) -> Option<RobotSummary> {
        let robots = self.project.backend.snapshot.read();
        let chosen = self.workspace.interaction.selected.read();
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
        let snapshot = self.project.backend.snapshot.read();
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
        self.workspace.backend.telemetry.read().recording().cloned()
    }

    /// The episode being played back right now.
    pub(crate) fn replay(&self) -> Option<ReplayProgress> {
        self.workspace.backend.telemetry.read().replay().cloned()
    }

    /// Robots whose motor bus is currently open.
    pub(crate) fn connected_robots(&self) -> Vec<RobotSummary> {
        self.project
            .backend
            .snapshot
            .read()
            .robots()
            .iter()
            .filter(|robot| robot.connection().connected())
            .cloned()
            .collect()
    }

    /// The mode the studio is in.
    pub(crate) fn mode(&self) -> ModeSelection {
        self.project
            .backend
            .snapshot
            .read()
            .mode()
            .selection()
            .to_owned()
    }

    /// Whether the configured mode is actually running.
    pub(crate) fn mode_active(&self) -> bool {
        self.project.backend.snapshot.read().mode().active()
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
    /// This remains available when telemetry reads fail; the backend still checks command admission.
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
        crate::client::project::ProjectClient::current().spawn(async move {
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

    fn notify(&self, notice: MessageNotice) {
        let mut notifications = self.project.notifications;
        notifications.write().push(notice, Self::now_ms());
    }
}

/// Read the shared studio state from context.
pub(crate) fn studio() -> Studio {
    use_context::<Studio>()
}

impl StudioProjectState {
    pub(crate) fn use_provider() -> Self {
        let state = Self {
            backend: ProjectBackendSignals {
                snapshot: use_signal(StudioSnapshot::default),
                live: use_signal(|| false),
                clock_ms: use_signal(|| 0.0),
            },
            resources: StudioProjectResources::use_resources(),
            notifications: use_signal(NotificationCenterState::default),
        };
        use_context_provider(|| state)
    }

    pub(crate) fn use_polling(mut self) {
        let sessions = ProjectSessionSignals::current();
        use_effect(move || self.backend.live.set(sessions.connected()));
        let setup = QueryPolicy::new(QueryDomain::Setup, 2_000)
            .use_resource(|| StudioClient::current().setup_snapshot());
        use_effect(move || {
            let result = setup.read();
            if setup.finished()
                && let Some(Ok(value)) = result.as_ref()
                && *self.backend.snapshot.peek() != *value
            {
                self.backend.snapshot.set(value.clone());
            }
        });
        use_future(move || async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(250).await;
                self.backend.clock_ms.set(Studio::now_ms());
            }
        });
    }

    pub(crate) fn loaded_settings(self) -> Option<DataSettings> {
        self.resources.loaded_settings.read().clone()
    }

    pub(crate) fn close_notifications(mut self) {
        self.notifications.write().close();
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::robots::{
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
                .map(|(index, name)| MotorSummary::new(*name, index as u32 + 1))
                .collect(),
        )
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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

    #[wasm_bindgen_test::wasm_bindgen_test]
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
