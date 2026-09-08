//! Semantic actions and backend consequences for the Live screen.

use crate::activities::settings::robots::RobotClient;
use dioxus::prelude::WritableExt;
use spiral_robotics_common::robots::ModeSelection;

use crate::activities::live::session::LiveSession;
use crate::shell::robot_workspace::Selection;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveAction {
    Select(Option<Selection>),
    Point(Option<Selection>),
    Hold(String),
    Release(String),
    Teleoperate { leader: String, follower: String },
}

impl LiveSession {
    pub(crate) fn dispatch_live_action(self, action: LiveAction) {
        match action {
            LiveAction::Select(selection) => self.selection().set(selection),
            LiveAction::Point(selection) => self.pointed_selection().set(selection),
            LiveAction::Hold(robot) => self.hold_robot(robot),
            LiveAction::Release(robot) => self.release_robot(robot),
            LiveAction::Teleoperate { leader, follower } => {
                self.set_mode(ModeSelection::Teleoperate { leader, follower });
            }
        }
    }

    fn hold_robot(self, id: String) {
        self.run("", async move {
            RobotClient::current()
                .hold_robot(id)
                .await
                .map_err(|error| error.to_string())
        });
    }

    fn release_robot(self, id: String) {
        self.run_safety("Motors released.", async move {
            RobotClient::current()
                .release_robot(id)
                .await
                .map_err(|error| error.to_string())
        });
    }

    fn set_mode(self, selection: ModeSelection) {
        let stops_motion = matches!(selection, ModeSelection::Idle);
        let future = async move {
            RobotClient::current()
                .set_mode(selection)
                .await
                .map_err(|error| error.to_string())
        };
        if stops_motion {
            self.run_safety(
                "Motion commands stopped. Motors may still be holding position.",
                future,
            );
        } else {
            self.run("", future);
        }
    }
}
