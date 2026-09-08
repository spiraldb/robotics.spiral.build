//! Rollout owns follower scene, checkpoint cameras, controls, and inference docking.

use super::workspace::{InferencePanel, RobotFleet, RolloutWorkbench, TrainingSnapshotRollout};
use crate::panels::camera_panel::{AssignedCamera, CameraPanelRows};
use crate::panels::scene::SceneSource;
use crate::panels::scene::{Viewport, ViewportPresentation};
use crate::shell::{frame::WorkspaceFrame, robot_workspace::RobotWorkspace};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

struct RolloutLayout;
impl RolloutLayout {
    fn initial(panels: &[Panel]) -> PanelLayout {
        let camera_ids = panels
            .iter()
            .filter(|panel| panel.home.as_str() == "cameras")
            .map(|panel| panel.id.clone())
            .collect::<Vec<_>>();
        let primary = LayoutNode::tile("primary", ["scene".to_owned()]);
        let editors = match camera_ids.camera_rows() {
            Some(cameras) => LayoutNode::split(
                "editor-columns",
                SplitAxis::Horizontal,
                2.0 / 3.0,
                primary,
                cameras,
            ),
            None => primary,
        };
        let visualization = LayoutNode::split(
            "visualization-rows",
            SplitAxis::Vertical,
            0.58,
            editors,
            LayoutNode::tile("timeline", ["rollout-inference".to_owned()]),
        );
        PanelLayout::new(LayoutNode::split(
            "studio-columns",
            SplitAxis::Horizontal,
            0.27,
            LayoutNode::tile("workbench", ["workbench".to_owned()]),
            visualization,
        ))
    }
}

struct RolloutCameras;
impl RolloutCameras {
    fn assigned(studio: RobotWorkspace) -> Vec<AssignedCamera> {
        let Some(follower) = studio.snapshot().read().robots().follower() else {
            return Vec::new();
        };
        let checkpoint =
            crate::activities::rollout::state::RolloutController::current().checkpoint();
        let roles = crate::activities::training::history::TrainingHistory::current()
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
    crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Rollout,
        is_active,
    );
    let robot = RobotWorkspace::use_provider(is_active);
    super::state::RolloutController::use_provider(is_active);
    crate::activities::training::history::TrainingHistory::use_history(is_active, || None);
    rsx! { WorkspaceFrame { status: crate::shell::frame::WorkspaceStatus::Hardware(robot), RolloutPanels {} } }
}

#[component]
fn RolloutPanels() -> Element {
    let studio = crate::shell::robot_workspace::RobotWorkspace::current();
    let mut resize_revision = use_signal(|| 0_u64);
    let mut selection = studio.selection();
    let follower = studio
        .snapshot()
        .read()
        .robots()
        .follower()
        .map(|robot| robot.id().to_owned());
    let cameras = RolloutCameras::assigned(studio);
    let mut panels = vec![
        Panel::new("workbench", "Rollout", "workbench", rsx! { aside { class: "studio-v2-workbench", RolloutWorkbench {} } }).with_class("workbench-editor"),
        Panel::new("scene", "Scene", "primary", rsx! {
            Viewport { source: SceneSource::Live, presentation: ViewportPresentation::new(follower, resize_revision()), on_select: move |next| selection.set(next) }
        }).with_class("scene-editor"),
    ];
    for camera in &cameras {
        panels.push(camera.panel(camera.live_source(), false));
    }
    if cameras.is_empty() {
        panels.push(AssignedCamera::unassigned_panel());
    }
    panels.push(
        Panel::new(
            "rollout-inference",
            "Inference",
            "timeline",
            rsx! { InferencePanel {} },
        )
        .with_class("rollout-inference-editor"),
    );
    let initial_layout = RolloutLayout::initial(&panels);
    rsx! {
        div { class: "min-h-0 min-w-0 overflow-hidden", tabindex: "-1",
            PanelWorkspace {
                panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v6.rollout".to_owned()),
                on_resize: move |_| { let next = resize_revision.peek().wrapping_add(1); resize_revision.set(next); },
                on_panel_activate: move |id: PanelId| {
                    if let Some(camera) = cameras.iter().find(|camera| camera.panel_id() == id) { selection.set(Some(camera.selection())); }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            crate::shell::robot_workspace::Selection::Camera {
                robot: "follower".to_owned(),
                role: "top".to_owned()
            }
        );
        assert_eq!(RolloutCameras::for_follower(&follower, &[]).len(), 3);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn rollout_layout_docks_inference_without_registering_a_recording_timeline() {
        let cameras = [AssignedCamera::unassigned_panel()];
        let layout = RolloutLayout::initial(&cameras);
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
