//! Semantic actions and backend consequences for the Live screen.

use dioxus::prelude::WritableExt;
use gloo_net::http::Method;
use spiral_robotics_contracts::api::ModeSelection;

use crate::frontend::api::http as api;
use crate::frontend::app::state::{Selection, Studio};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveAction {
    Select(Option<Selection>),
    Point(Option<Selection>),
    Hold(String),
    Release(String),
    Teleoperate { leader: String, follower: String },
}

impl Studio {
    pub(crate) fn dispatch_live_action(mut self, action: LiveAction) {
        match action {
            LiveAction::Select(selection) => self.selected.set(selection),
            LiveAction::Point(selection) => self.pointed.set(selection),
            LiveAction::Hold(robot) => self.hold_robot(robot),
            LiveAction::Release(robot) => self.release_robot(robot),
            LiveAction::Teleoperate { leader, follower } => {
                self.set_mode(ModeSelection::Teleoperate { leader, follower });
            }
        }
    }

    fn hold_robot(self, id: String) {
        self.run("", async move {
            api::post(&format!("/api/robots/{id}/hold")).await
        });
    }

    fn release_robot(self, id: String) {
        self.run_safety("Motors released.", async move {
            api::post(&format!("/api/robots/{id}/release")).await
        });
    }

    fn set_mode(self, selection: ModeSelection) {
        let request = selection.clone();
        let future = async move { api::send(Method::POST, "/api/mode", Some(&request)).await };
        if matches!(selection, ModeSelection::Idle) {
            self.run_safety(
                "Motion commands stopped. Motors may still be holding position.",
                future,
            );
        } else {
            self.run("", future);
        }
    }
}
