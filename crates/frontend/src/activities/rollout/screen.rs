//! Route-owned Rollout panel registry and follower-focused composition.

use dioxus::prelude::*;
use dioxus_workbench::{Panel, PanelLayout};

use crate::shell::activity::PanelRole;
use crate::shell::layout::PanelLayoutDefinition;
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
        let layout: PanelLayout = layout_panels.default_panel_layout_with_workbench(0.27);
        Self::new(panels, layout, "spiral_robotics.panel-layout.v6.rollout")
    }
}

#[component]
pub(crate) fn RolloutWorkspace() -> Element {
    let studio = crate::shell::state::Studio::use_studio(crate::shell::activity::Activity::Rollout);
    studio.use_hardware_polling();
    studio.use_rollout_polling();
    rsx! {
        crate::shell::frame::WorkspaceFrame {
            crate::shell::workspace::EpisodeWorkspace { activity: crate::shell::activity::Activity::Rollout }
        }
    }
}
