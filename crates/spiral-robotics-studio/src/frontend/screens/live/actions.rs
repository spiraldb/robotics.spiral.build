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

pub(crate) fn dispatch_live_action(mut studio: Studio, action: LiveAction) {
    match action {
        LiveAction::Select(selection) => studio.selected.set(selection),
        LiveAction::Point(selection) => studio.pointed.set(selection),
        LiveAction::Hold(robot) => hold_robot(studio, robot),
        LiveAction::Release(robot) => release_robot(studio, robot),
        LiveAction::Teleoperate { leader, follower } => {
            set_mode(studio, ModeSelection::Teleoperate { leader, follower });
        }
    }
}

fn hold_robot(studio: Studio, id: String) {
    studio.run("", async move {
        api::post(&format!("/api/robots/{id}/hold")).await
    });
}

fn release_robot(studio: Studio, id: String) {
    studio.run_safety("Motors released.", async move {
        api::post(&format!("/api/robots/{id}/release")).await
    });
}

fn set_mode(studio: Studio, selection: ModeSelection) {
    let request = selection.clone();
    let future = async move { api::send(Method::POST, "/api/mode", Some(&request)).await };
    if matches!(selection, ModeSelection::Idle) {
        studio.run_safety(
            "Motion commands stopped. Motors may still be holding position.",
            future,
        );
    } else {
        studio.run("", future);
    }
}
