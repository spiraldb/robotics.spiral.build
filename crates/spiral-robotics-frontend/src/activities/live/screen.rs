//! Route-owned Live panel registry and first-open composition.

use dioxus_workbench::{Panel, PanelLayout};

use crate::shell::activity::PanelRole;
use crate::shell::layout::PanelLayoutDefinition;
use crate::shell::workspace::ScreenWorkspaceDefinition;

const LIVE_WORKBENCH_FRACTION: f64 = 0.29;

impl ScreenWorkspaceDefinition {
    pub(crate) fn for_live(
        workbench: Panel,
        mut evidence: Vec<(Panel, PanelRole)>,
        timeline: Panel,
    ) -> Self {
        let mut panels = Vec::with_capacity(evidence.len() + 2);
        panels.push((workbench, PanelRole::Workbench));
        panels.append(&mut evidence);
        panels.push((timeline, PanelRole::Timeline));
        let layout_panels = panels
            .iter()
            .map(|(panel, _)| panel.clone())
            .collect::<Vec<_>>();
        let layout: PanelLayout =
            layout_panels.default_panel_layout_with_workbench(LIVE_WORKBENCH_FRACTION);
        Self::new(panels, layout, "spiral_robotics.panel-layout.v7.live")
    }
}
