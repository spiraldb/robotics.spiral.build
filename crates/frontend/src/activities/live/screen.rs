//! Live owns recording controls, scene, cameras, timeline, and docking.

use super::playback::LiveTimeline;
use super::session::LiveSession;
use super::workbench::LiveWorkbench;
use crate::panels::camera_panel::{AssignedCamera, CameraPanelRows};
use crate::panels::scene::{Viewport, ViewportPresentation};
use crate::robots::{HardwareTelemetry, RobotSelection};
use crate::shell::frame::WorkspaceFrame;
use crate::shell::{project::ProjectSessionSignals, state::StudioProjectState};
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

struct LiveLayout;
impl LiveLayout {
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
            LayoutNode::tile("timeline", ["timeline".to_owned()]),
        );
        PanelLayout::new(LayoutNode::split(
            "studio-columns",
            SplitAxis::Horizontal,
            0.29,
            LayoutNode::tile("workbench", ["workbench".to_owned()]),
            visualization,
        ))
    }
}

#[component]
pub(crate) fn LiveWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Live,
        is_active,
    );
    let project = StudioProjectState::current();
    let telemetry =
        HardwareTelemetry::use_telemetry(project, is_active, ProjectSessionSignals::current());
    let selection = RobotSelection::use_selection(project.snapshot());
    let session = LiveSession::use_session(project, telemetry, selection);
    rsx! { WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Hardware(session.telemetry()), LivePanels { session } } }
}

#[component]
fn LivePanels(session: LiveSession) -> Element {
    let timeline = LiveTimeline::use_timeline(session);
    let actions = timeline.actions();
    let mut resize_revision = use_signal(|| 0_u64);
    let mut selection = session.selection().selected();
    let cameras = session
        .selection()
        .focused(session.project().snapshot().read().robots())
        .map(|robot| {
            robot
                .cameras()
                .iter()
                .map(|binding| AssignedCamera::from((&robot, binding)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut panels = vec![
        Panel::new("workbench", "Live", "workbench", rsx! {
            aside { class: "studio-v2-workbench", LiveWorkbench { project: session.project(), hardware: session.telemetry(), selection: session.selection(), datasets: session.datasets(), on_action: move |action| session.dispatch_live_action(action) } }
        }).with_class("workbench-editor"),
        Panel::new("scene", "Scene", "primary", rsx! {
            Viewport { project: session.project(), telemetry: session.telemetry(), robot_selection: session.selection(), source: timeline.scene_source(), presentation: ViewportPresentation::new(None, resize_revision()) }
        }).with_class("scene-editor"),
    ];
    for camera in &cameras {
        panels.push(camera.panel(timeline.camera_source(camera), timeline.media_playing()));
    }
    if cameras.is_empty() {
        panels.push(AssignedCamera::unassigned_panel());
    }
    panels.push(
        Panel::new("timeline", "Timeline", "timeline", timeline.render())
            .with_class("timeline-editor"),
    );
    let initial_layout = LiveLayout::initial(&panels);
    rsx! {
        div { class: "min-h-0 min-w-0 overflow-hidden", tabindex: "-1", onkeydown: move |event| actions.on_key(event),
            PanelWorkspace {
                panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v7.live".to_owned()),
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn live_layout_preserves_named_homes_when_camera_panels_change() {
        let panels = [
            Panel::new("camera:follower:wrist", "Wrist", "cameras", rsx! {}),
            Panel::new("camera:follower:top", "Top", "cameras", rsx! {}),
        ];
        let layout = LiveLayout::initial(&panels);
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
        let mut restored: PanelLayout =
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
