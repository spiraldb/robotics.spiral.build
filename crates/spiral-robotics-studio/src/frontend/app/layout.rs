//! Stable topology for the frontend Studio panel shell.
//!
//! Feature registries provide panel identities and preferred homes. This module
//! alone decides the first-open split tree; `spiral-robotics-panels` owns subsequent
//! reconciliation and persisted operator edits.

use spiral_robotics_panels::{LayoutNode, Panel, PanelId, PanelLayout, SplitAxis};

/// The canonical first-open arrangement. Persistence takes over after this,
/// while panel `home` ids tell hot-plugged content where it belongs.
pub(super) fn default_panel_layout(panels: &[Panel]) -> PanelLayout {
    default_panel_layout_with_workbench(panels, 1.0 / 3.0)
}

/// Rollout gives the scene more room than the editing-heavy activities while
/// keeping the same durable dock topology.
pub(super) fn default_panel_layout_with_workbench(
    panels: &[Panel],
    workbench_fraction: f64,
) -> PanelLayout {
    let ids_in = |home: &str| {
        panels
            .iter()
            .filter(|panel| panel.home.as_str() == home)
            .map(|panel| panel.id.clone())
            .collect::<Vec<_>>()
    };
    let primary = LayoutNode::tile("primary", ids_in("primary"));
    let cameras = ids_in("cameras");
    let editors = if let Some(camera_stack) = default_camera_stack(&cameras) {
        LayoutNode::split(
            "editor-columns",
            SplitAxis::Horizontal,
            2.0 / 3.0,
            primary,
            camera_stack,
        )
    } else {
        primary
    };
    let visualization = LayoutNode::split(
        "visualization-rows",
        SplitAxis::Vertical,
        0.58,
        editors,
        LayoutNode::tile("timeline", ids_in("timeline")),
    );
    PanelLayout::new(LayoutNode::split(
        "studio-columns",
        SplitAxis::Horizontal,
        workbench_fraction,
        LayoutNode::tile("workbench", ids_in("workbench")),
        visualization,
    ))
}

/// Give every camera its own visible row on first open. The horizontal
/// dividers preserve a useful 16:9 width in the right-hand camera column; the
/// panel workspace still owns every later tab, dock, and resize mutation.
fn default_camera_stack(cameras: &[PanelId]) -> Option<LayoutNode> {
    fn build(cameras: &[PanelId], offset: usize) -> LayoutNode {
        let tile_id = if offset == 0 {
            "cameras".to_owned()
        } else {
            format!("cameras-{}", offset + 1)
        };
        let first = LayoutNode::tile(tile_id, [cameras[0].clone()]);
        if cameras.len() == 1 {
            return first;
        }

        LayoutNode::split(
            format!("camera-rows-{}", offset + 1),
            SplitAxis::Vertical,
            1.0 / cameras.len() as f64,
            first,
            build(&cameras[1..], offset + 1),
        )
    }

    (!cameras.is_empty()).then(|| build(cameras, 0))
}
