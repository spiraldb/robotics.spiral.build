//! Rollout owns follower scene, checkpoint cameras, controls, and inference docking.

use super::workspace::{InferencePanel, RobotFleet, RolloutWorkbench, TrainingSnapshotRollout};
use crate::panels::camera_panel::{AssignedCamera, CameraWorkspaceLayout};
use crate::panels::scene::SceneSource;
use crate::panels::scene::{Viewport, ViewportPresentation};
use crate::robots::{HardwareViewState, RobotSelection};
use crate::shell::{
    frame::WorkspaceFrame, project::ProjectSessionSignals, state::StudioProjectState,
};
use dioxus::prelude::*;
use dioxus_workbench::{Panel, PanelWorkspace};

struct RolloutCameras;
impl RolloutCameras {
    fn assigned(
        telemetry: HardwareViewState,
        controller: super::state::RolloutController,
        training_history: crate::activities::training::history::TrainingHistory,
    ) -> Vec<AssignedCamera> {
        let Some(follower) = telemetry.snapshot().read().robots().follower() else {
            return Vec::new();
        };
        let checkpoint = controller.checkpoint();
        let roles = training_history
            .snapshot()
            .read()
            .selected_checkpoint_camera_roles(checkpoint.as_deref());
        Self::for_follower(&follower, &roles)
    }

    fn for_follower(
        follower: &spiral_robotics_common::robots::RobotSummary,
        roles: &[String],
    ) -> Vec<AssignedCamera> {
        if roles.is_empty() {
            follower
                .cameras()
                .iter()
                .map(|binding| AssignedCamera::from((follower, binding)))
                .collect()
        } else {
            roles
                .iter()
                .filter_map(|role| {
                    follower
                        .cameras()
                        .iter()
                        .find(|binding| binding.name() == role.as_str())
                })
                .map(|binding| AssignedCamera::from((follower, binding)))
                .collect()
        }
    }
}

#[component]
pub(crate) fn RolloutWorkspace(is_active: ReadSignal<bool>) -> Element {
    let (workspace, project) = crate::shell::activity::WorkspaceActivity::mount(
        crate::shell::activity::Activity::Rollout,
        is_active,
    );
    let telemetry =
        HardwareViewState::use_state(project, is_active, ProjectSessionSignals::current());
    let robot_selection = RobotSelection::use_selection(telemetry.snapshot());
    let controller = super::state::RolloutController::use_controller(is_active);
    let training_history =
        crate::activities::training::history::TrainingHistory::use_history(is_active, || None);
    rsx! { WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Hardware(telemetry), RolloutPanels { project, telemetry, robot_selection, controller, training_history } } }
}

#[component]
fn RolloutPanels(
    project: StudioProjectState,
    telemetry: HardwareViewState,
    robot_selection: RobotSelection,
    controller: super::state::RolloutController,
    training_history: crate::activities::training::history::TrainingHistory,
) -> Element {
    let selection = robot_selection.selected();
    let follower = telemetry
        .snapshot()
        .read()
        .robots()
        .follower()
        .map(|robot| robot.id().to_owned());
    let cameras = RolloutCameras::assigned(telemetry, controller, training_history);
    let mut panels = vec![
        Panel::new("workbench", "Rollout", "workbench", rsx! { aside { class: "studio-v2-workbench", RolloutWorkbench { project, telemetry, controller, training_history } } }).with_class("workbench-editor"),
        Panel::new("scene", "Scene", "primary", rsx! {
            Viewport { telemetry, robot_selection, source: SceneSource::Live, presentation: ViewportPresentation::new(follower) }
        }).with_class("scene-editor"),
    ];
    for camera in &cameras {
        panels.push(camera.panel(camera.live_source()));
    }
    if cameras.is_empty() {
        panels.push(AssignedCamera::unassigned_panel());
    }
    panels.push(
        Panel::new(
            "rollout-inference",
            "Inference",
            "timeline",
            rsx! { InferencePanel { controller } },
        )
        .with_class("rollout-inference-editor"),
    );
    let initial_layout = CameraWorkspaceLayout::initial(&panels, "rollout-inference", 0.27);
    rsx! {
        div { class: "min-h-0 min-w-0 overflow-hidden", tabindex: "-1",
            PanelWorkspace {
                panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v6.rollout".to_owned()),
                on_panel_activate: CameraWorkspaceLayout::on_activate(cameras, selection),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_workbench::PanelId;
    use spiral_robotics_common::robots::{CameraBinding, RobotAvailability, RobotSummary};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_camera_panels_follow_checkpoint_order_and_keep_robot_identity() {
        let follower = RobotSummary::new(
            "follower",
            Some("so101_follower".to_owned()),
            RobotAvailability::default(),
        )
        .with_cameras(vec![
            CameraBinding::new("wrist", "camera-1", None),
            CameraBinding::new("top", "camera-2", None),
            CameraBinding::new("extra", "camera-3", None),
        ]);
        let cameras = RolloutCameras::for_follower(
            &follower,
            &["top".to_owned(), "missing".to_owned(), "wrist".to_owned()],
        );
        assert_eq!(
            cameras
                .iter()
                .map(AssignedCamera::panel_id)
                .collect::<Vec<_>>(),
            vec![
                PanelId::from("camera:follower:top"),
                PanelId::from("camera:follower:wrist")
            ]
        );
        assert_eq!(
            cameras[0].selection(),
            crate::robots::Selection::Camera {
                robot: "follower".to_owned(),
                role: "top".to_owned()
            }
        );
        assert_eq!(RolloutCameras::for_follower(&follower, &[]).len(), 3);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_layout_docks_inference_without_registering_a_recording_timeline() {
        let cameras = [AssignedCamera::unassigned_panel()];
        let layout = CameraWorkspaceLayout::initial(&cameras, "rollout-inference", 0.27);
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("rollout-inference"))
                .unwrap()
                .as_str(),
            "timeline"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("camera-empty"))
                .unwrap()
                .as_str(),
            "cameras"
        );
        assert!(layout.tile_for_panel(&PanelId::from("timeline")).is_none());
    }
}
