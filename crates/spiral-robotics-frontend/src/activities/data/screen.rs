//! Route-owned Data panel registry and first-open composition.

#[cfg(test)]
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, SplitAxis};

use crate::shell::activity::PanelRole;
use crate::shell::workspace::ScreenWorkspaceDefinition;

pub(crate) struct DataWorkbenchPanels {
    episodes: Panel,
    query: Panel,
}

impl DataWorkbenchPanels {
    pub(crate) fn new(episodes: Panel, query: Panel) -> Self {
        Self { episodes, query }
    }
}

pub(crate) struct DataContextPanels {
    contextual_evidence: Vec<(Panel, PanelRole)>,
    timeline: Panel,
}

impl DataContextPanels {
    pub(crate) fn new(contextual_evidence: Vec<(Panel, PanelRole)>, timeline: Panel) -> Self {
        Self {
            contextual_evidence,
            timeline,
        }
    }
}

pub(crate) struct DataEvidencePanels {
    query_result: Panel,
    relations: Panel,
    context: DataContextPanels,
}

impl DataEvidencePanels {
    pub(crate) fn new(query_result: Panel, relations: Panel, context: DataContextPanels) -> Self {
        Self {
            query_result,
            relations,
            context,
        }
    }
}

/// Episodes and Query are independent top-level panels. Relations shares the
/// primary evidence tile as a tab so the user can move between an episode and
/// the whole project's table structure without losing either context.
impl ScreenWorkspaceDefinition {
    pub(crate) fn for_data(workbench: DataWorkbenchPanels, evidence: DataEvidencePanels) -> Self {
        let DataWorkbenchPanels { episodes, query } = workbench;
        let DataEvidencePanels {
            query_result,
            relations,
            context,
        } = evidence;
        let DataContextPanels {
            mut contextual_evidence,
            timeline,
        } = context;
        let mut panels = vec![
            (episodes, PanelRole::Workbench),
            (query, PanelRole::Workbench),
            (query_result, PanelRole::PrimaryVisualization),
            (relations, PanelRole::PrimaryVisualization),
        ];
        panels.append(&mut contextual_evidence);
        panels.push((timeline, PanelRole::Timeline));

        let panel_ids = panels
            .iter()
            .map(|(panel, _)| panel.id.as_str().to_owned())
            .collect::<Vec<_>>();
        let has = |id: &str| panel_ids.iter().any(|candidate| candidate == id);
        let primary_id = if has("episode-scene") {
            "episode-scene"
        } else {
            "episodes-empty"
        };
        let camera_id = panel_ids
            .iter()
            .find(|id| {
                id.starts_with("episode-camera:")
                    || id.as_str() == "data-camera-context"
                    || id.as_str() == "episode-camera-empty"
            })
            .cloned()
            .unwrap_or_else(|| "data-camera-context".to_owned());

        let tools = LayoutNode::split(
            "data-tools",
            SplitAxis::Vertical,
            0.58,
            LayoutNode::tile("episodes", ["episodes".to_owned()]),
            LayoutNode::tile("query", ["query".to_owned()]),
        );
        let evidence_top = LayoutNode::split(
            "data-evidence-top",
            SplitAxis::Horizontal,
            0.68,
            LayoutNode::tile("primary", [primary_id.to_owned(), "relations".to_owned()]),
            LayoutNode::tile("cameras", [camera_id]),
        );
        let evidence_bottom = LayoutNode::split(
            "data-evidence-bottom",
            SplitAxis::Horizontal,
            0.58,
            LayoutNode::tile("query-result", ["query-result".to_owned()]),
            LayoutNode::tile("timeline", ["timeline".to_owned()]),
        );
        let evidence = LayoutNode::split(
            "data-evidence",
            SplitAxis::Vertical,
            0.56,
            evidence_top,
            evidence_bottom,
        );
        let layout = PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.29,
            tools,
            evidence,
        ));

        Self::new(panels, layout, "spiral_robotics.panel-layout.v6.data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_workbench::PanelId;

    #[test]
    fn episodes_query_and_results_have_independent_default_tiles() {
        let definition = ScreenWorkspaceDefinition::for_data(
            DataWorkbenchPanels::new(
                Panel::new("episodes", "Episodes", "episodes", rsx! { div {} }),
                Panel::new("query", "Query", "query", rsx! { div {} }),
            ),
            DataEvidencePanels::new(
                Panel::new("query-result", "Results", "query-result", rsx! { div {} }),
                Panel::new("relations", "Relations", "primary", rsx! { div {} }),
                DataContextPanels::new(
                    vec![
                        (
                            Panel::new("episodes-empty", "Scene", "primary", rsx! { div {} }),
                            PanelRole::PrimaryVisualization,
                        ),
                        (
                            Panel::new("data-camera-context", "Camera", "cameras", rsx! { div {} }),
                            PanelRole::Placeholder,
                        ),
                    ],
                    Panel::new("timeline", "Timeline", "timeline", rsx! { div {} }),
                ),
            ),
        );

        let tile = |id| {
            definition
                .initial_layout()
                .tile_for_panel(&PanelId::from(id))
                .map(|tile| tile.as_str().to_owned())
        };
        assert_eq!(tile("episodes").as_deref(), Some("episodes"));
        assert_eq!(tile("query").as_deref(), Some("query"));
        assert_eq!(tile("query-result").as_deref(), Some("query-result"));
        assert_eq!(tile("relations").as_deref(), Some("primary"));
    }
}
