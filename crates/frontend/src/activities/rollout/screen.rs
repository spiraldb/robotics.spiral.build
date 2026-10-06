//! Rollout owns the drivers, recording controls, scene, cameras, timeline,
//! and docking, and, until a follower is ready, the guide that gets it there.

use super::playback::TakeTimeline;
use super::session::RolloutSession;
use super::setup::BenchSetupView;
use super::workbench::RolloutWorkbench;
use crate::panels::camera_panel::{AssignedCamera, CameraWorkspaceLayout};
use crate::panels::scene::{Viewport, ViewportPresentation};
use crate::robots::arm_setup::{ArmSetupGuide, BenchSetup, BenchStage};
use crate::robots::{HardwareViewState, RobotSelection};
use crate::shell::activity::{Activity, WorkspaceActivity};
use crate::shell::frame::{WorkspaceFrame, WorkspaceStatus};
use crate::shell::notifications::MessageSource;
use crate::shell::routes::StudioRoute;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use dioxus_workbench::{Panel, PanelWorkspace};
use spiral_robotics_ui::{EmptyState, SpinnerIcon};

#[component]
pub(crate) fn RolloutWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = WorkspaceActivity::provide(Activity::Rollout, is_active);
    let project = StudioProjectState::current();
    let telemetry = HardwareViewState::use_state(project, is_active);
    let selection = RobotSelection::use_selection(telemetry.snapshot());
    let session = RolloutSession::use_session(project, telemetry, selection);
    let gate = BenchGate::use_gate(project, telemetry);
    let setup_open = use_route::<StudioRoute>() == (StudioRoute::RolloutSetup {});
    let (status, content) = if gate.showing_bench() {
        (
            WorkspaceStatus::Hardware(telemetry),
            rsx! { RolloutPanels { session, setup_open } },
        )
    } else if gate.stage() == BenchStage::Checking {
        (
            WorkspaceStatus::General,
            rsx! { EmptyState { icon: rsx! { SpinnerIcon {} }, p { "Looking for the robot connector…" } } },
        )
    } else {
        (
            WorkspaceStatus::General,
            rsx! { ArmSetupGuide { project, telemetry, selection, on_skip: move |_| gate.skip() } },
        )
    };
    rsx! { WorkspaceFrame { workspace, status, {content} } }
}

/// Whether Rollout shows its bench or the setup guide.
///
/// Once the bench shows it stays for this visit: a robot connector that stops
/// answering mid-session is the status line's to report, not a reason to take
/// the controls away.
#[derive(Clone, Copy)]
struct BenchGate {
    stage: Memo<BenchStage>,
    bench_shown: Signal<bool>,
}

impl BenchGate {
    fn use_gate(project: StudioProjectState, telemetry: HardwareViewState) -> Self {
        let connector = crate::client::robot_connector::RobotConnector::current();
        let stage = use_memo(move || {
            let health = connector.health();
            let snapshot = telemetry.snapshot();
            let snapshot = snapshot.read();
            let robots = telemetry.setup().loaded().then(|| snapshot.robots());
            BenchSetup::new(health.as_ref(), robots).stage()
        });
        let mut bench_shown = use_signal(|| false);
        let mut guided = use_signal(|| false);
        use_effect(move || match stage() {
            BenchStage::Ready if !*bench_shown.peek() => {
                bench_shown.set(true);
                if *guided.peek() {
                    let teleop = telemetry
                        .snapshot()
                        .peek()
                        .default_teleoperation_pair()
                        .is_some();
                    project.report_success_from(
                        MessageSource::Rollout,
                        if teleop {
                            "Both arms are ready. Choose Teleop to drive the follower from the leader."
                        } else {
                            "The follower is ready. Set up a leader to drive it by hand."
                        },
                    );
                }
            }
            BenchStage::Setup if !*guided.peek() => guided.set(true),
            _ => {}
        });
        Self { stage, bench_shown }
    }

    fn stage(self) -> BenchStage {
        (self.stage)()
    }

    fn showing_bench(self) -> bool {
        (self.bench_shown)() || self.stage() == BenchStage::Ready
    }

    /// Show the bench without finishing setup.
    fn skip(mut self) {
        self.bench_shown.set(true);
    }
}

#[component]
fn RolloutPanels(session: RolloutSession, setup_open: bool) -> Element {
    let timeline = TakeTimeline::use_timeline(session);
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
    // Setup opens in the workbench, beside the scene and cameras it sets up.
    let workbench = if setup_open {
        rsx! { BenchSetupView { session } }
    } else {
        rsx! { RolloutWorkbench { session } }
    };
    let mut panels = vec![
        Panel::new("workbench", "Rollout", "workbench", rsx! {
            aside { class: "studio-v2-workbench", {workbench} }
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
                panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v8.rollout".to_owned()),
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
    fn rollout_layout_preserves_named_homes_when_camera_panels_change() {
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
