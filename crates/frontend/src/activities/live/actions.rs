//! Semantic actions and backend consequences for the Live screen.

use crate::robots::RobotClient;
use dioxus::prelude::WritableExt;
use spiral_robotics_common::robots::ModeSelection;

use crate::activities::live::session::LiveSession;
use crate::robots::Selection;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveAction {
    Select(Option<Selection>),
    Point(Option<Selection>),
    Hold(String),
    Release(String),
    Teleoperate { leader: String, follower: String },
}

impl LiveSession {
    pub(super) fn dispatch_live_action(self, action: LiveAction) {
        match action {
            LiveAction::Select(selection) => self.selection().selected().set(selection),
            LiveAction::Point(selection) => self.selection().pointed().set(selection),
            LiveAction::Hold(robot) => self.hold_robot(robot),
            LiveAction::Release(robot) => self.release_robot(robot),
            LiveAction::Teleoperate { leader, follower } => self.teleoperate(leader, follower),
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
        self.commands().run("", async move {
            RobotClient::current()
                .set_mode(ModeSelection::Teleoperate { leader, follower })
                .await
                .map_err(|error| error.to_string())
        });
    }
}
