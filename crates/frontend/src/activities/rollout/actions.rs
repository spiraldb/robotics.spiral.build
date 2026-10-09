//! Semantic actions and robot connector consequences for the Rollout screen.

use crate::robots::RobotClient;
use dioxus::prelude::WritableExt;
use spiral_robotics_common::robots::ModeSelection;

use super::next_episode::ControlKind;
use super::policy::PolicyControl;
use crate::activities::rollout::session::RolloutSession;
use crate::robots::Selection;
use crate::shell::notifications::MessageSource;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RolloutAction {
    Select(Option<Selection>),
    Point(Option<Selection>),
    /// Hold an arm where it is.
    Hold(String),
    Release(String),
    Teleoperate {
        leader: String,
        follower: String,
    },
    /// Run the chosen checkpoint on the follower; not supported yet.
    RunPolicy,
}

impl RolloutSession {
    pub(super) fn dispatch(self, action: RolloutAction) {
        match action {
            RolloutAction::Select(selection) => self.selection().selected().set(selection),
            RolloutAction::Point(selection) => self.selection().pointed().set(selection),
            RolloutAction::Hold(robot) => self.hold_robot(robot),
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
        self.next_episode().choose_control(ControlKind::Teleop);
        self.commands().run("", async move {
            RobotClient::current()
                .set_mode(ModeSelection::Teleoperate { leader, follower })
                .await
                .map_err(|error| error.to_string())
        });
    }

    /// Running a checkpoint on the follower is not supported yet.
    fn run_policy(self) {
        self.project()
            .report_from(MessageSource::Rollout, PolicyControl::UNSUPPORTED);
    }
}
