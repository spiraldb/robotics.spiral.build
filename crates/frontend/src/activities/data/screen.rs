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

pub(crate) struct DataEvidencePanels {
    query_result: Panel,
    relations: Panel,
}

impl DataEvidencePanels {
    pub(crate) fn new(query_result: Panel, relations: Panel) -> Self {
        Self {
            query_result,
            relations,
        }
    }
}

/// Native datasets lead to topic previews in Results. SQL remains visible below,
/// and Relations shares the Results tile for project-level inspection.
impl ScreenWorkspaceDefinition {
    pub(crate) fn for_data(workbench: DataWorkbenchPanels, evidence: DataEvidencePanels) -> Self {
        let DataWorkbenchPanels { episodes, query } = workbench;
        let DataEvidencePanels {
            query_result,
            relations,
        } = evidence;
        let panels = vec![
            (episodes, PanelRole::Workbench),
            (query, PanelRole::Workbench),
            (query_result, PanelRole::PrimaryVisualization),
            (relations, PanelRole::PrimaryVisualization),
        ];
        let tools = LayoutNode::tile("episodes", ["episodes".to_owned()]);
        let evidence = LayoutNode::split(
            "data-evidence",
            SplitAxis::Vertical,
            0.68,
            LayoutNode::tile(
                "query-result",
                ["query-result".to_owned(), "relations".to_owned()],
            ),
            LayoutNode::tile("query", ["query".to_owned()]),
        );
        let layout = PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.29,
            tools,
            evidence,
        ));

        Self::new(panels, layout, "spiral_robotics.panel-layout.v7.data")
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
        assert_eq!(tile("relations").as_deref(), Some("query-result"));
    }
}
