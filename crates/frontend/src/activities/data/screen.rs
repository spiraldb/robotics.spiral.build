//! Route-owned Data panel registry and first-open composition.

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
    inspector: Panel,
    query_result: Panel,
    relations: Panel,
}

impl DataEvidencePanels {
    pub(crate) fn new(inspector: Panel, query_result: Panel, relations: Panel) -> Self {
        Self {
            inspector,
            query_result,
            relations,
        }
    }
}

/// Dataset navigation leads to the inspector. SQL, query results, and project
/// tables remain available as secondary tabs, with native docking preserved.
impl ScreenWorkspaceDefinition {
    pub(crate) fn for_data(workbench: DataWorkbenchPanels, evidence: DataEvidencePanels) -> Self {
        let DataWorkbenchPanels { episodes, query } = workbench;
        let DataEvidencePanels {
            inspector,
            query_result,
            relations,
        } = evidence;
        let panels = vec![
            (inspector, PanelRole::PrimaryVisualization),
            (episodes, PanelRole::Workbench),
            (query, PanelRole::Workbench),
            (query_result, PanelRole::PrimaryVisualization),
            (relations, PanelRole::PrimaryVisualization),
        ];
        let tools = LayoutNode::tile("episodes", ["episodes".to_owned()]);
        let evidence = LayoutNode::tile(
            "dataset-inspector",
            [
                "dataset-inspector".to_owned(),
                "query".to_owned(),
                "query-result".to_owned(),
                "relations".to_owned(),
            ],
        );
        let layout = PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.24,
            tools,
            evidence,
        ));

        Self::new(panels, layout, "spiral_robotics.panel-layout.v8.data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_workbench::PanelId;

    #[test]
    fn dataset_inspection_leads_and_sql_tools_remain_available() {
        let definition = ScreenWorkspaceDefinition::for_data(
            DataWorkbenchPanels::new(
                Panel::new("episodes", "Episodes", "episodes", rsx! { div {} }),
                Panel::new("query", "Query", "query", rsx! { div {} }),
            ),
            DataEvidencePanels::new(
                Panel::new(
                    "dataset-inspector",
                    "Inspect",
                    "dataset-inspector",
                    rsx! { div {} },
                ),
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
        assert_eq!(
            tile("dataset-inspector").as_deref(),
            Some("dataset-inspector")
        );
        assert_eq!(tile("query").as_deref(), Some("dataset-inspector"));
        assert_eq!(tile("query-result").as_deref(), Some("dataset-inspector"));
        assert_eq!(tile("relations").as_deref(), Some("dataset-inspector"));
    }
}

#[component]
pub(crate) fn DataWorkspace() -> Element {
    let studio = crate::shell::state::Studio::use_studio(crate::shell::activity::Activity::Data);
    studio.use_relation_catalog();
    let project = consume_context::<crate::shell::state::StudioProjectState>();
    use_effect(move || {
        if let Some(episode) = project.take_requested_episode() {
            studio.data_controller().set_selected_episode(Some(episode));
        }
    });
    rsx! {
        crate::shell::frame::WorkspaceFrame {
            crate::shell::workspace::EpisodeWorkspace { activity: crate::shell::activity::Activity::Data }
        }
    }
}
