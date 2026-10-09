//! Semantic actions and robot connector consequences for the Rollout screen.

use crate::robots::RobotClient;
use dioxus::prelude::{ReadableExt, WritableExt};
use spiral_robotics_common::robots::ModeSelection;

use super::next_take::ControlKind;
use super::policy::PolicyClient;
use crate::activities::rollout::session::RolloutSession;
use crate::robots::Selection;
use crate::shell::notifications::MessageSource;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RolloutAction {
    Select(Option<Selection>),
    Point(Option<Selection>),
    /// Hold an arm where it is; while a policy drives, hold stops the policy.
    Hold(String),
    Release(String),
    Teleoperate {
        leader: String,
        follower: String,
    },
    /// Run the chosen checkpoint on the follower.
    RunPolicy,
}

impl RolloutSession {
    pub(super) fn dispatch(self, action: RolloutAction) {
        match action {
            RolloutAction::Select(selection) => self.selection().selected().set(selection),
            RolloutAction::Point(selection) => self.selection().pointed().set(selection),
            RolloutAction::Hold(robot) => match self.policy().active_session() {
                Some(session) => self.stop_policy(session),
                None => self.hold_robot(robot),
            },
            RolloutAction::Release(robot) => self.release_robot(robot),
            RolloutAction::Teleoperate { leader, follower } => self.teleoperate(leader, follower),
            RolloutAction::RunPolicy => self.run_policy(),
        }
    }

    fn hold_robot(self, id: String) {
        self.commands().run("", async move {
            RobotClient::current()
                .hold_robot(id)
                .await
                .map_err(|error| error.to_string())
        });
    }

    fn release_robot(self, id: String) {
        self.project().run_reported("Motors released.", async move {
            RobotClient::current()
                .release_robot(id)
                .await
                .map_err(|error| error.to_string())
        });
    }

    fn teleoperate(self, leader: String, follower: String) {
        self.next_take().choose_control(ControlKind::Teleop);
        self.commands().run("", async move {
            RobotClient::current()
                .set_mode(ModeSelection::Teleoperate { leader, follower })
                .await
                .map_err(|error| error.to_string())
        });
    }

    /// Run the chosen checkpoint on the follower, which the scene and cameras
    /// then follow.
    fn run_policy(self) {
        if let Some(reason) = self.policy_blocker() {
            self.project().report_from(MessageSource::Rollout, reason);
            return;
        }
        let follower = self
            .telemetry()
            .snapshot()
            .read()
            .follower()
            .map(|robot| robot.id().to_owned());
        let Some((follower, config)) =
            follower.and_then(|id| Some((id.clone(), self.policy().next_run(&id)?)))
        else {
            return;
        };
        self.next_take().choose_control(ControlKind::Policy);
        self.selection()
            .selected()
            .set(Some(Selection::Robot(follower)));
        self.commands().run("", async move {
            PolicyClient::current()
                .start(config)
                .await
                .map_err(|error| error.to_string())
        });
    }

    fn stop_policy(self, session: String) {
        self.project().run_reported("", async move {
            PolicyClient::current()
                .stop(session)
                .await
                .map_err(|error| error.to_string())
        });
    }
}
