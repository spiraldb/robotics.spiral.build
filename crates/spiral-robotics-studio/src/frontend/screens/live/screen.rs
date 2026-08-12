//! Route-owned Live panel registry and first-open composition.

use dioxus::prelude::*;
use spiral_robotics_panels::{Panel, PanelLayout};

use crate::frontend::app::activity::{Activity, PanelRole};
use crate::frontend::app::layout::default_panel_layout;
use crate::frontend::app::workspace::{EpisodeWorkspace, ScreenWorkspaceDefinition};

#[component]
pub(crate) fn LiveScreen() -> Element {
    rsx! { EpisodeWorkspace { activity: Activity::Live } }
}

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
    let layout: PanelLayout = default_panel_layout(&layout_panels);
    ScreenWorkspaceDefinition::new(panels, layout, "spiral_robotics.panel-layout.v6.live")
}
