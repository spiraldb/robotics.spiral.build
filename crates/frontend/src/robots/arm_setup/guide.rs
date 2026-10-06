//! What Rollout shows a bench that is not ready yet: start the robot
//! connector, then bring a follower, and a leader if there is one, to a port
//! and a calibration.
//!
//! Every step is read from the robot connector, never remembered here, so
//! leaving Rollout halfway and coming back resumes where the bench actually
//! is. The steps are the same sections Rollout's Setup shows, in the order the
//! work happens.

use dioxus::prelude::*;
use spiral_robotics_common::robot_connector::RobotConnectorHealth;
use spiral_robotics_common::robots::RobotSummary;
use spiral_robotics_common::robots::config::MotorAccess;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, CheckIcon, TextInput};

use super::calibration::CalibrationSection;
use super::{
    ImportRobotConfiguration, NewArm, PortCatalogState, PortSection, PortSectionState,
    RobotConnectorSetup,
};
use crate::client::CommandError;
use crate::robots::{HardwareViewState, RobotSelection, Selection};
use crate::shell::state::StudioProjectState;

/// What Rollout has to show for the bench it can see.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BenchStage {
    /// The robot connector has not answered yet, or its robots are loading.
    Checking,
    /// Something is missing: the guide shows what.
    Setup,
    /// A follower has a port and a calibration, and so does a leader if one
    /// was started. A policy needs only the follower.
    Ready,
}

/// What the robot connector has said about the bench.
pub(crate) struct BenchSetup<'a> {
    health: Option<&'a Result<RobotConnectorHealth, CommandError>>,
    robots: Option<&'a [RobotSummary]>,
}

impl<'a> BenchSetup<'a> {
    /// `robots` is `None` until the robot connector's robots have loaded.
    pub(crate) fn new(
        health: Option<&'a Result<RobotConnectorHealth, CommandError>>,
        robots: Option<&'a [RobotSummary]>,
    ) -> Self {
        Self { health, robots }
    }

    pub(crate) fn stage(&self) -> BenchStage {
        match (self.health, self.robots) {
            (None, _) | (Some(Ok(_)), None) => BenchStage::Checking,
            (Some(Err(_)), _) => BenchStage::Setup,
            (Some(Ok(_)), Some(robots)) => {
                let leader_settled = ArmRole::Leader
                    .find(robots)
                    .is_none_or(RobotSummary::ready_to_drive);
                if ArmRole::Follower.ready_in(robots) && leader_settled {
                    BenchStage::Ready
                } else {
                    BenchStage::Setup
                }
            }
        }
    }
}

/// One of the two arms Rollout drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArmRole {
    Follower,
    Leader,
}

impl ArmRole {
    const fn robot_type(self) -> &'static str {
        match self {
            Self::Follower => "so101_follower",
            Self::Leader => "so101_leader",
        }
    }

    const fn default_id(self) -> &'static str {
        match self {
            Self::Follower => "follower",
            Self::Leader => "leader",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Follower => "Set up the follower",
            Self::Leader => "Set up a leader",
        }
    }

    const fn purpose(self) -> &'static str {
        match self {
            Self::Follower => {
                "The arm that moves on its own. Teleop mirrors the leader onto it, and policies drive it."
            }
            Self::Leader => {
                "The arm you move by hand to teleoperate the follower. A policy needs only the follower."
            }
        }
    }

    fn holds(self, robot: &RobotSummary) -> bool {
        let wanted = match self {
            Self::Follower => "follower",
            Self::Leader => "leader",
        };
        robot
            .config()
            .robot_type()
            .is_some_and(|kind| kind.to_ascii_lowercase().contains(wanted))
    }

    /// The arm this role is set up on: a ready one if there is one, otherwise
    /// the first one started.
    fn find(self, robots: &[RobotSummary]) -> Option<&RobotSummary> {
        let mut arms = robots.iter().filter(|robot| self.holds(robot));
        arms.clone()
            .find(|robot| robot.ready_to_drive())
            .or_else(|| arms.next())
    }

    fn ready_in(self, robots: &[RobotSummary]) -> bool {
        self.find(robots).is_some_and(RobotSummary::ready_to_drive)
    }
}

/// Whether an arm is set up: it has a motor bus and a calibration. Being
/// connected right now is the robot connector's business, not setup's.
trait ArmReadiness {
    fn ready_to_drive(&self) -> bool;
}

impl ArmReadiness for RobotSummary {
    fn ready_to_drive(&self) -> bool {
        self.config().motor_access().is_some() && self.config().calibrated()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepState {
    Pending,
    Current,
    Done,
}

impl StepState {
    const fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Current => "current",
            Self::Done => "done",
        }
    }
}

/// The robot connector, then the follower, and a leader alongside it.
#[component]
pub(crate) fn ArmSetupGuide(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    on_skip: EventHandler<()>,
) -> Element {
    let connector = crate::client::robot_connector::RobotConnector::current();
    let answering = connector.reachable();
    let robots = telemetry.snapshot().read().robots().to_owned();
    let port_catalog = PortCatalogState::use_catalog();
    let arms = ArmSteps {
        project,
        telemetry,
        selection,
        port_catalog,
    };
    let follower = ArmRole::Follower.find(&robots).cloned();
    let leader = ArmRole::Leader.find(&robots).cloned();
    let follower_ready = follower.as_ref().is_some_and(RobotSummary::ready_to_drive);
    let leader_ready = leader.as_ref().is_some_and(RobotSummary::ready_to_drive);
    let connector_summary = match connector.health() {
        Some(Ok(health)) => Some(format!(
            "{} · {}",
            connector
                .address()
                .trim_start_matches("http://")
                .trim_start_matches("https://"),
            health.version()
        )),
        _ => None,
    };
    let arm_state = |ready: bool, reachable: bool| match (ready, reachable) {
        (true, _) => StepState::Done,
        (false, true) => StepState::Current,
        (false, false) => StepState::Pending,
    };

    rsx! {
        div { class: "arm-guide",
            div { class: "arm-guide-sheet",
                header { class: "arm-guide-head",
                    div {
                        h2 { "Get your arms ready" }
                        p {
                            "Rollout drives the follower, from a leader you move or from a policy, through the robot connector on the computer the arms are plugged into. "
                            "Plan on about ten minutes, most of it calibration."
                        }
                    }
                    Button {
                        purpose: ActionPurpose::Close,
                        variant: ButtonVariant::Link,
                        onclick: move |_| on_skip.call(()),
                        "Skip setup"
                    }
                }
                ol { class: "arm-guide-steps",
                    GuideStep {
                        number: 1,
                        title: "Start the robot connector",
                        state: if answering { StepState::Done } else { StepState::Current },
                        summary: connector_summary,
                        p { class: "arm-guide-purpose",
                            "It runs on the computer your arms are plugged into, and keeps their ports, calibration, and recordings."
                        }
                        RobotConnectorSetup {}
                    }
                    GuideStep {
                        number: 2,
                        title: ArmRole::Follower.title(),
                        state: arm_state(follower_ready, answering),
                        summary: follower.as_ref().filter(|_| follower_ready).map(ArmSummary::summary),
                        ArmStep { arms, role: ArmRole::Follower, robot: follower.clone() }
                    }
                    GuideStep {
                        number: 3,
                        title: ArmRole::Leader.title(),
                        state: arm_state(leader_ready, answering),
                        summary: leader.as_ref().filter(|_| leader_ready).map(ArmSummary::summary),
                        ArmStep { arms, role: ArmRole::Leader, robot: leader.clone() }
                    }
                }
            }
        }
    }
}

/// One row of the guide: its mark, its title, what it settled on once done,
/// and its work while it is the current step.
#[component]
fn GuideStep(
    number: u8,
    title: &'static str,
    state: StepState,
    summary: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        li {
            class: "arm-guide-step",
            "data-state": state.name(),
            "aria-current": (state == StepState::Current).then_some("step"),
            div { class: "arm-guide-step-head",
                span { class: "arm-guide-mark", "aria-hidden": "true",
                    if state == StepState::Done {
                        CheckIcon {}
                    } else {
                        "{number}"
                    }
                }
                h3 { "{title}" }
                if let Some(summary) = summary {
                    span { class: "arm-guide-summary", "{summary}" }
                }
            }
            if state == StepState::Current {
                div { class: "arm-guide-body", {children} }
            }
        }
    }
}

/// What every arm step works with.
#[derive(Clone, Copy, PartialEq)]
struct ArmSteps {
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    port_catalog: PortCatalogState,
}

/// Add the arm, then give it a port and a calibration.
#[component]
fn ArmStep(arms: ArmSteps, role: ArmRole, robot: Option<RobotSummary>) -> Element {
    let ArmSteps {
        project,
        telemetry,
        selection,
        port_catalog,
    } = arms;
    let Some(robot) = robot else {
        return rsx! {
            p { class: "arm-guide-purpose", "{role.purpose()}" }
            NewArmForm { project, selection, role }
        };
    };
    let sweeping = telemetry
        .robot_telemetry(robot.id())
        .and_then(|telemetry| telemetry.calibration().cloned())
        .is_some();
    rsx! {
        p { class: "arm-guide-purpose",
            "{role.purpose()} Plug in its USB cable and power, then choose its port; Identify finds it if you are not sure which."
        }
        div { class: "arm-guide-sections",
            div { class: "device-setup-section",
                PortSection { project, telemetry,
                    robot: robot.clone(),
                    state: PortSectionState::new(
                        port_catalog.scan(),
                        sweeping,
                        move |()| port_catalog.rescan(),
                    ),
                }
            }
            div { class: "device-setup-section",
                CalibrationSection { project, telemetry, robot }
            }
        }
    }
}

/// Name the arm and add it; the role decides its type.
#[component]
fn NewArmForm(project: StudioProjectState, selection: RobotSelection, role: ArmRole) -> Element {
    let mut chosen = selection.selected();
    let mut name = use_signal(|| role.default_id().to_owned());
    let mut add = use_action(move |id: String| async move {
        project
            .execute_utility(format!("Added {id}."), async move {
                let saved = NewArm::new(id, role.robot_type()).save().await?;
                chosen.set(Some(Selection::Robot(saved)));
                Ok(())
            })
            .await
    });
    let mut submit = move || {
        let id = name.read().trim().to_owned();
        if id.is_empty() {
            project.report("Give the arm a name first.");
        } else {
            add.call(id);
        }
    };
    rsx! {
        div { class: "arm-guide-new",
            TextInput {
                aria_label: "Arm name",
                value: name(),
                disabled: add.pending(),
                oninput: move |value| name.set(value),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter {
                        submit();
                    }
                },
            }
            Button {
                purpose: ActionPurpose::Create,
                variant: ButtonVariant::Solid,
                disabled: add.pending(),
                aria_busy: add.pending(),
                onclick: move |_| submit(),
                if add.pending() { "Adding…" } else { "Add {role.default_id()}" }
            }
            ImportRobotConfiguration { selection, project }
        }
    }
}

/// How a finished arm step reads: the arm and the motor bus it settled on.
trait ArmSummary {
    fn summary(&self) -> String;
}

impl ArmSummary for RobotSummary {
    fn summary(&self) -> String {
        match self.config().motor_access() {
            Some(MotorAccess::Serial(port)) => format!("{} · {}", self.id(), port.path()),
            Some(MotorAccess::Simulated(_)) => format!("{} · simulated", self.id()),
            None => self.id().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::command_failure::CommandFailureCode;
    use spiral_robotics_common::robots::ConnectionStatus;
    use spiral_robotics_common::robots::config::RobotConfig;

    struct BenchFixture;

    impl BenchFixture {
        fn arm(id: &str, robot_type: &str, port: bool, calibrated: bool) -> RobotSummary {
            let calibration = serde_json::json!({"range_min": 0, "center": 1, "range_max": 2});
            let mut document = serde_json::json!({
                "robot_id": id,
                "robot_type": robot_type,
                "motors": {"shoulder": {"id": 1}},
            });
            if port {
                document["port"] =
                    serde_json::json!({"path": format!("/dev/{id}"), "baud_rate": 1_000_000});
            }
            if calibrated {
                document["motors"]["shoulder"]["calibration"] = calibration;
            }
            RobotSummary::new(
                serde_json::from_value::<RobotConfig>(document).unwrap(),
                1,
                ConnectionStatus::default(),
            )
        }

        fn answering() -> Result<RobotConnectorHealth, CommandError> {
            Ok(RobotConnectorHealth::new("0.1.0"))
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_bench_is_ready_with_a_follower_and_any_leader_it_started() {
        let answering = BenchFixture::answering();
        let unreachable = Err(CommandError::new(CommandFailureCode::Transport, "refused"));
        let ready = [
            BenchFixture::arm("follower", "so101_follower", true, true),
            BenchFixture::arm("leader", "so101_leader", true, true),
        ];
        assert_eq!(BenchSetup::new(None, None).stage(), BenchStage::Checking);
        assert_eq!(
            BenchSetup::new(Some(&answering), None).stage(),
            BenchStage::Checking
        );
        assert_eq!(
            BenchSetup::new(Some(&unreachable), Some(&ready)).stage(),
            BenchStage::Setup
        );
        assert_eq!(
            BenchSetup::new(Some(&answering), Some(&ready)).stage(),
            BenchStage::Ready
        );
        for unfinished in [
            BenchFixture::arm("leader", "so101_leader", false, true),
            BenchFixture::arm("leader", "so101_leader", true, false),
        ] {
            let robots = [ready[0].clone(), unfinished];
            assert_eq!(
                BenchSetup::new(Some(&answering), Some(&robots)).stage(),
                BenchStage::Setup
            );
        }
        assert_eq!(
            BenchSetup::new(Some(&answering), Some(&ready[..1])).stage(),
            BenchStage::Ready
        );
        assert_eq!(
            BenchSetup::new(Some(&answering), Some(&ready[1..])).stage(),
            BenchStage::Setup
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_role_is_set_up_on_its_ready_arm_before_an_unfinished_one() {
        let robots = [
            BenchFixture::arm("spare", "so101_follower", false, false),
            BenchFixture::arm("main", "so101_follower", true, true),
            BenchFixture::arm("leader", "so101_leader", false, false),
        ];
        assert_eq!(ArmRole::Follower.find(&robots).unwrap().id(), "main");
        assert_eq!(ArmRole::Leader.find(&robots).unwrap().id(), "leader");
        assert!(ArmRole::Leader.find(&robots[..2]).is_none());
    }
}
