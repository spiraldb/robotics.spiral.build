//! Stable topology for the frontend Studio panel shell.
//!
//! Feature registries provide panel identities and preferred homes. This module
//! alone decides the first-open split tree; `dioxus-workbench` owns subsequent
//! reconciliation and persisted operator edits.

use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, SplitAxis};

/// The canonical first-open arrangement. Persistence takes over after this,
/// while panel `home` ids tell hot-plugged content where it belongs.
pub(crate) trait PanelLayoutDefinition {
    #[cfg(test)]
    fn default_panel_layout(&self) -> PanelLayout;
    fn default_panel_layout_with_workbench(&self, workbench_fraction: f64) -> PanelLayout;
}

impl PanelLayoutDefinition for [Panel] {
    #[cfg(test)]
    fn default_panel_layout(&self) -> PanelLayout {
        self.default_panel_layout_with_workbench(1.0 / 3.0)
    }

    /// Rollout gives the scene more room than the editing-heavy activities while
    /// keeping the same durable dock topology.
    fn default_panel_layout_with_workbench(&self, workbench_fraction: f64) -> PanelLayout {
        let ids_in = |home: &str| {
            self.iter()
                .filter(|panel| panel.home.as_str() == home)
                .map(|panel| panel.id.clone())
                .collect::<Vec<_>>()
        };
        let primary = LayoutNode::tile("primary", ids_in("primary"));
        let cameras = ids_in("cameras");
        let editors = if let Some(camera_stack) = cameras.camera_stack() {
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
}

/// Give every camera its own visible row on first open. The horizontal
/// dividers preserve a useful 16:9 width in the right-hand camera column; the
/// panel workspace still owns every later tab, dock, and resize mutation.
trait CameraPanelStack {
    fn camera_stack(&self) -> Option<LayoutNode>;
    fn camera_rows(&self, offset: usize) -> LayoutNode;
}

impl CameraPanelStack for [PanelId] {
    fn camera_stack(&self) -> Option<LayoutNode> {
        (!self.is_empty()).then(|| self.camera_rows(0))
    }

    fn camera_rows(&self, offset: usize) -> LayoutNode {
        let tile_id = if offset == 0 {
            "cameras".to_owned()
        } else {
            format!("cameras-{}", offset + 1)
        };
        let first = LayoutNode::tile(tile_id, [self[0].clone()]);
        if self.len() == 1 {
            return first;
        }

        LayoutNode::split(
            format!("camera-rows-{}", offset + 1),
            SplitAxis::Vertical,
            1.0 / self.len() as f64,
            first,
            self[1..].camera_rows(offset + 1),
        )
    }
}
