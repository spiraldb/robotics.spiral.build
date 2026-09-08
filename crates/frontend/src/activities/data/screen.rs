//! Dataset navigation, inspection, and SQL panel composition owned by Data.

use super::native_datasets::{
    DatasetBrowserSelection, NativeDatasetInspector, NativeDatasetsPanel,
};
use super::query_result::QueryResultView;
use super::relations::RelationsPanel;
use super::workbench::QueryPanel;
use crate::shell::{activity::Activity, frame::WorkspaceFrame, state::Studio};
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
    crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Data,
        is_active,
    );
    let studio = Studio::use_studio(Activity::Data);
    studio.use_relation_catalog(is_active);
    rsx! { WorkspaceFrame { DataPanels {} } }
}

#[component]
fn DataPanels() -> Element {
    let studio = crate::shell::state::studio();
    let browser = DatasetBrowserSelection::provide();
    let configuration = studio.configuration();
    let namespace = configuration.dataset();
    let dataset_id = studio.selected_dataset().read().to_owned();
    let query_available = configuration.spiral_query_available();
    let panels = vec![
        Panel::new(
            "dataset-inspector",
            "Inspect",
            "dataset-inspector",
            rsx! { NativeDatasetInspector {} },
        ),
        Panel::new(
            "episodes",
            "Datasets",
            "episodes",
            rsx! { NativeDatasetsPanel {} },
        )
        .with_class("episodes-workbench-editor"),
        Panel::new(
            "query",
            "SQL",
            "query",
            rsx! { QueryPanel { namespace, dataset_id, query_available } },
        )
        .with_class("query-workbench-editor"),
        Panel::new(
            "query-result",
            "Results",
            "query-result",
            rsx! { QueryResultView {} },
        )
        .with_class("query-editor"),
        Panel::new(
            "relations",
            "Tables",
            "primary",
            rsx! { RelationsPanel { namespace, query_available } },
        )
        .with_class("relations-editor"),
    ];
    let active_panel = if studio.data_controller().query_running() {
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
