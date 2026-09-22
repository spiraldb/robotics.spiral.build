//! Live owns recording controls, scene, cameras, timeline, and docking.

use super::playback::LiveTimeline;
use super::session::LiveSession;
use super::workbench::LiveWorkbench;
use crate::panels::camera_panel::{AssignedCamera, CameraWorkspaceLayout};
use crate::panels::scene::{Viewport, ViewportPresentation};
use crate::robots::{HardwareViewState, RobotSelection};
use crate::shell::activity::{Activity, WorkspaceActivity};
use crate::shell::frame::WorkspaceFrame;
use dioxus::prelude::*;
use dioxus_workbench::{Panel, PanelWorkspace};

#[component]
pub(crate) fn LiveWorkspace(is_active: ReadSignal<bool>) -> Element {
    let (workspace, project) = WorkspaceActivity::mount(Activity::Live, is_active);
    let telemetry = HardwareViewState::use_state(project, is_active);
    let selection = RobotSelection::use_selection(telemetry.snapshot());
    let session = LiveSession::use_session(project, telemetry, selection);
    rsx! { WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Hardware(session.telemetry()), LivePanels { session } } }
}

#[component]
fn LivePanels(session: LiveSession) -> Element {
    let timeline = LiveTimeline::use_timeline(session);
    let transport = timeline.transport();
    let selection = session.selection().selected();
    let cameras = session
        .selection()
        .focused(session.telemetry().snapshot().read().robots())
        .map(|robot| {
            robot
                .config()
                .cameras()
                .iter()
                .map(|camera| AssignedCamera::from((&robot, camera)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut panels = vec![
        Panel::new("workbench", "Live", "workbench", rsx! {
            aside { class: "studio-v2-workbench", LiveWorkbench { project: session.project(), hardware: session.telemetry(), selection: session.selection(), capture: session.capture(), on_action: move |action| session.dispatch_live_action(action) } }
        }).with_class("workbench-editor"),
        Panel::new("scene", "Scene", "primary", rsx! {
            Viewport { telemetry: session.telemetry(), robot_selection: session.selection(), source: timeline.scene_source(), presentation: ViewportPresentation::new(None) }
        }).with_class("scene-editor"),
    ];
    for camera in &cameras {
        panels.push(camera.panel(timeline.camera_source(camera)));
    }
    if cameras.is_empty() {
        panels.push(AssignedCamera::unassigned_panel());
    }
    panels.push(
        Panel::new("timeline", "Timeline", "timeline", timeline.render())
            .with_class("timeline-editor"),
    );
    let initial_layout = CameraWorkspaceLayout::initial(&panels, "timeline", 0.29);
    rsx! {
        div { class: "min-h-0 min-w-0 overflow-hidden", tabindex: "-1", onkeydown: move |event| transport.on_key(event),
            PanelWorkspace {
                panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v7.live".to_owned()),
                on_panel_activate: CameraWorkspaceLayout::on_activate(cameras, selection),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_workbench::PanelId;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn live_layout_preserves_named_homes_when_camera_panels_change() {
        let panels = [
            Panel::new("camera:follower:wrist", "Wrist", "cameras", rsx! {}),
            Panel::new("camera:follower:top", "Top", "cameras", rsx! {}),
        ];
        let layout = CameraWorkspaceLayout::initial(&panels, "timeline", 0.29);
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("camera:follower:wrist"))
                .unwrap()
                .as_str(),
            "cameras"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("camera:follower:top"))
                .unwrap()
                .as_str(),
            "cameras-2"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("scene"))
                .unwrap()
                .as_str(),
            "primary"
        );
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("timeline"))
                .unwrap()
                .as_str(),
            "timeline"
        );
        let mut restored: dioxus_workbench::PanelLayout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        restored.reconcile(&[
            dioxus_workbench::PanelPlacement::new("workbench", "workbench"),
            dioxus_workbench::PanelPlacement::new("scene", "primary"),
            dioxus_workbench::PanelPlacement::new("camera:follower:top", "cameras"),
            dioxus_workbench::PanelPlacement::new("timeline", "timeline"),
        ]);
        assert_eq!(
            restored.tile_for_panel(&PanelId::from("camera:follower:top")),
            layout.tile_for_panel(&PanelId::from("camera:follower:top"))
        );
        assert!(
            restored
                .tile_for_panel(&PanelId::from("camera:follower:wrist"))
                .is_none()
        );
    }
}
