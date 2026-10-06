//! Setting the bench up: the robot connector this workspace drives through,
//! and each arm's port, cameras, motors, and calibration. It opens in the
//! workbench, beside the scene and cameras it configures.

use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

use super::session::RolloutSession;
use crate::client::robot_connector::RobotConnector;
use crate::robots::arm_setup::{RobotConnectorSetup, RobotPanel};
use crate::shell::routes::StudioRoute;

#[component]
pub(super) fn BenchSetupView(session: RolloutSession) -> Element {
    let navigator = use_navigator();
    rsx! {
        div { class: "studio-workbench-scroll rollout-setup",
            header { class: "workbench-section-heading",
                h3 { "Setup" }
                Button {
                    purpose: ActionPurpose::Close,
                    variant: ButtonVariant::Outline,
                    class: "workbench-heading-action",
                    onclick: move |_| { navigator.push(StudioRoute::Rollout {}); },
                    "Done"
                }
            }
            section { class: "rollout-setup-connector",
                RobotConnectorSetup {}
            }
            if RobotConnector::current().reachable() {
                section { class: "rollout-setup-arms",
                    RobotPanel {
                        project: session.project(),
                        telemetry: session.telemetry(),
                        selection: session.selection(),
                    }
                }
            }
        }
    }
}
