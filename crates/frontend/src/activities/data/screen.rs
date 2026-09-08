//! Dataset navigation, inspection, and SQL panel composition owned by Data.

use super::native_datasets::{
    DatasetBrowserSelection, NativeDatasetInspector, NativeDatasetsPanel,
};
use super::query_result::QueryResultView;
use super::relations::RelationsPanel;
use super::workbench::QueryPanel;
use crate::shell::frame::WorkspaceFrame;
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

struct DataLayout;
impl DataLayout {
    fn initial() -> PanelLayout {
        PanelLayout::new(LayoutNode::split(
            "data-columns",
            SplitAxis::Horizontal,
            0.24,
            LayoutNode::tile("episodes", ["episodes".to_owned()]),
            LayoutNode::tile(
                "dataset-inspector",
                [
                    "dataset-inspector".to_owned(),
                    "query".to_owned(),
                    "query-result".to_owned(),
                    "relations".to_owned(),
                ],
            ),
        ))
    }
}

#[component]
pub(crate) fn DataWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Data,
        is_active,
    );
    rsx! { WorkspaceFrame { workspace, DataPanels {} } }
}

#[component]
fn DataPanels() -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let controller = super::state::QueryController::use_controller();
    let catalog = super::relations::RelationCatalogState::use_catalog();
    let browser = DatasetBrowserSelection::use_selection();
    let panels = vec![
        Panel::new(
            "dataset-inspector",
            "Inspect",
            "dataset-inspector",
            rsx! { NativeDatasetInspector { browser } },
        ),
        Panel::new(
            "episodes",
            "Datasets",
            "episodes",
            rsx! { NativeDatasetsPanel { browser } },
        )
        .with_class("episodes-workbench-editor"),
        Panel::new(
            "query",
            "SQL",
            "query",
            rsx! { QueryPanel { studio, controller, catalog } },
        )
        .with_class("query-workbench-editor"),
        Panel::new(
            "query-result",
            "Results",
            "query-result",
            rsx! { QueryResultView { controller } },
        )
        .with_class("query-editor"),
        Panel::new(
            "relations",
            "Tables",
            "primary",
            rsx! { RelationsPanel { studio, controller, catalog } },
        )
        .with_class("relations-editor"),
    ];
    let active_panel = if controller.query_running() {
        Some(PanelId::from("query-result"))
    } else {
        browser.requested_panel()
    };
    rsx! {
        div { class: "min-h-0 min-w-0 overflow-hidden", tabindex: "-1",
            PanelWorkspace {
                panels, initial_layout: DataLayout::initial(),
                layout_scope: Some("spiral_robotics.panel-layout.v8.data".to_owned()),
                active_panel,
                on_panel_activate: move |_| browser.clear_request(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn dataset_inspection_leads_and_sql_tools_remain_available() {
        let layout = DataLayout::initial();
        for id in ["dataset-inspector", "query", "query-result", "relations"] {
            assert_eq!(
                layout.tile_for_panel(&PanelId::from(id)).unwrap().as_str(),
                "dataset-inspector"
            );
        }
        assert_eq!(
            layout
                .tile_for_panel(&PanelId::from("episodes"))
                .unwrap()
                .as_str(),
            "episodes"
        );
    }
}
