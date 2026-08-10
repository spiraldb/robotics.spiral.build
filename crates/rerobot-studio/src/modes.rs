//! Shared client transition for modes that still have a local control.

use gloo_net::http::Method;
use rerobot_core::api::ModeSelection;

use crate::api;
use crate::state::Studio;

pub fn set_mode(studio: Studio, selection: ModeSelection) {
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
