//! Route-owned Rollout panel registry and follower-focused composition.

use dioxus_workbench::{Panel, PanelLayout};

use crate::shell::activity::PanelRole;
use crate::shell::layout::default_panel_layout_with_workbench;
use crate::shell::workspace::ScreenWorkspaceDefinition;

impl ScreenWorkspaceDefinition {
    pub(crate) fn for_rollout(
        workbench: Panel,
        mut evidence: Vec<(Panel, PanelRole)>,
        inference: Panel,
    ) -> Self {
        let mut panels = Vec::with_capacity(evidence.len() + 2);
        panels.push((workbench, PanelRole::Workbench));
        panels.append(&mut evidence);
        panels.push((inference, PanelRole::RolloutInference));
        let layout_panels = panels
            .iter()
            .map(|(panel, _)| panel.clone())
            .collect::<Vec<_>>();
        let layout: PanelLayout = default_panel_layout_with_workbench(&layout_panels, 0.27);
        Self::new(panels, layout, "spiral_robotics.panel-layout.v6.rollout")
    }
}
