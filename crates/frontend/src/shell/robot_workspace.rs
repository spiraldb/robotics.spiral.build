//! Hardware readings and robot selection owned by each hardware workspace.
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::{StudioClient, project::ProjectSessionSignals, state::StudioProjectState};
use dioxus::prelude::*;
use spiral_robotics_common::recording::{RecordingProgress, ReplayProgress};
use spiral_robotics_common::robots::{RobotSummary, RobotTelemetry, TelemetryFrame};
use std::future::Future;
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RobotWorkspace {
    project: StudioProjectState,
    telemetry: Signal<TelemetryFrame>,
    last_telemetry_ms: Signal<f64>,
    selected: Signal<Option<Selection>>,
    pointed: Signal<Option<Selection>>,
    is_active: ReadSignal<bool>,
}
impl std::ops::Deref for RobotWorkspace {
    type Target = StudioProjectState;
    fn deref(&self) -> &Self::Target {
        &self.project
    }
}
impl RobotWorkspace {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn use_provider(is_active: ReadSignal<bool>) -> Self {
        let project = StudioProjectState::current();
        let workspace = use_context_provider(|| Self {
            project,
            telemetry: Signal::new(TelemetryFrame::default()),
            last_telemetry_ms: Signal::new(0.0),
            selected: Signal::new(None),
            pointed: Signal::new(None),
            is_active,
        });
        let mut selected = workspace.selected;
        use_effect(move || {
            let snapshot = project.snapshot();
            let next = snapshot
                .read()
                .robots()
                .default_joint_selection(selected.read().as_ref());
            if let Some(next) = next {
                selected.set(Some(next));
            }
        });
        workspace.use_hardware_polling(is_active);
        workspace
    }
    pub(crate) fn telemetry(self) -> MappedSignal<TelemetryFrame, Signal<TelemetryFrame>> {
        self.telemetry.map(|value| value)
    }
    pub(crate) const fn selection(self) -> Signal<Option<Selection>> {
        self.selected
    }
    pub(crate) const fn pointed_selection(self) -> Signal<Option<Selection>> {
        self.pointed
    }
    fn use_hardware_polling(mut self, is_active: ReadSignal<bool>) {
        // Reactivation must receive a fresh sample before enabling controls.
        use_effect(move || {
            if !is_active() {
                self.last_telemetry_ms.set(0.0);
            }
        });
        use_drop(move || {
            self.last_telemetry_ms.set(0.0);
        });
        let telemetry = QueryPolicy::new(QueryDomain::Telemetry, 50).use_when(
            move || *is_active.read(),
            || StudioClient::current().telemetry_snapshot(),
        );
        use_effect(move || {
            let result = telemetry.read();
            if telemetry.finished()
                && let Some(Ok(frame)) = result.as_ref()
                && self.telemetry.peek().sequence() != frame.sequence()
            {
                self.last_telemetry_ms.set(StudioProjectState::now_ms());
                self.telemetry.set(frame.clone());
            }
        });
    }
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
    pub(crate) fn controls_available(&self) -> bool {
        let age = self.telemetry_age_seconds();
        (self.is_active)()
            && (self.project.live())()
            && ProjectSessionSignals::current().catalog_available()
            && age.is_finite()
            && age < 2.5
    }
    pub(crate) fn telemetry_age_seconds(&self) -> f64 {
        let last = (self.last_telemetry_ms)();
        if last <= 0.0 {
            return f64::INFINITY;
        }
        (((self.project.clock_ms())() - last) / 1000.0).max(0.0)
    }
    pub(crate) fn focused(&self) -> Option<RobotSummary> {
        let snapshot = self.project.snapshot();
        let robots = snapshot.read();
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
    pub(crate) fn recording(&self) -> Option<RecordingProgress> {
        self.telemetry.read().recording().cloned()
    }
    pub(crate) fn replay(&self) -> Option<ReplayProgress> {
        self.telemetry.read().replay().cloned()
    }
    pub(crate) fn run<F>(&self, success: impl Into<String>, future: F)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        if !self.controls_available() {
            self.report("Controls are paused because telemetry is unavailable or delayed. No command was sent.");
            return;
        }
        self.project.run_utility(success, future);
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
