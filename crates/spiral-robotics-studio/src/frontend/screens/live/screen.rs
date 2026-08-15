//! Route-owned Live panel registry and first-open composition.

use spiral_robotics_panels::{Panel, PanelLayout};

use crate::frontend::app::activity::PanelRole;
use crate::frontend::app::layout::default_panel_layout_with_workbench;
use crate::frontend::app::workspace::ScreenWorkspaceDefinition;

const LIVE_WORKBENCH_FRACTION: f64 = 0.29;

pub(crate) fn compose(
    workbench: Panel,
    mut evidence: Vec<(Panel, PanelRole)>,
    timeline: Panel,
) -> ScreenWorkspaceDefinition {
    let mut panels = Vec::with_capacity(evidence.len() + 2);
    panels.push((workbench, PanelRole::Workbench));
    panels.append(&mut evidence);
    panels.push((timeline, PanelRole::Timeline));
    let layout_panels = panels
        .iter()
        .map(|(panel, _)| panel.clone())
        .collect::<Vec<_>>();
    let layout: PanelLayout =
        default_panel_layout_with_workbench(&layout_panels, LIVE_WORKBENCH_FRACTION);
    ScreenWorkspaceDefinition::new(panels, layout, "spiral_robotics.panel-layout.v7.live")
}
