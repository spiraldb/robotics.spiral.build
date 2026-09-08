//! Project catalog beside independent object sessions.
use super::{
    DatasetClient, native_datasets::NativeDatasetInspector, query_result::QueryResultView,
    state::QueryController, workbench::QueryPanel,
};
use crate::shell::frame::WorkspaceFrame;
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::native_datasets::ProjectResource;

#[derive(Clone, PartialEq)]
pub(super) struct OpenResource {
    resource: ProjectResource,
    topic: Option<String>,
    revision: u64,
}
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ResourceTabs {
    opened: Signal<Vec<OpenResource>>,
    active: Signal<Option<String>>,
    importing: Signal<bool>,
}
impl ResourceTabs {
    fn use_tabs() -> Self {
        let opened = use_signal(Vec::new);
        let active = use_signal(|| None);
        let importing = use_signal(|| false);
        Self {
            opened,
            active,
            importing,
        }
    }
    pub(super) fn open(mut self, resource: ProjectResource, topic: Option<String>) {
        self.importing.set(false);
        let id = resource.resource_id().to_owned();
        let mut opened = self.opened.write();
        if let Some(tab) = opened
            .iter_mut()
            .find(|tab| tab.resource.resource_id() == id)
        {
            tab.topic = topic;
            tab.revision = tab.revision.wrapping_add(1);
        } else {
            opened.push(OpenResource {
                resource,
                topic,
                revision: 0,
            });
        }
        drop(opened);
        self.active.set(Some(id));
    }
    fn close(mut self, id: &str) {
        let mut opened = self.opened.write();
        let Some(index) = opened
            .iter()
            .position(|tab| tab.resource.resource_id() == id)
        else {
            return;
        };
        opened.remove(index);
        if self.active.peek().as_deref() == Some(id) {
            self.active.set(
                opened
                    .get(index)
                    .or_else(|| opened.last())
                    .map(|tab| tab.resource.resource_id().to_owned()),
            );
        }
    }
    pub(super) fn active_id(self) -> Option<String> {
        self.active.read().clone()
    }
    fn navigate(mut self, key: Key) {
        let opened = self.opened.read();
        if opened.is_empty() {
            return;
        }
        let index = opened
            .iter()
            .position(|tab| Some(tab.resource.resource_id()) == self.active.peek().as_deref())
            .unwrap_or(0);
        let next = match key {
            Key::ArrowLeft => (index + opened.len() - 1) % opened.len(),
            Key::ArrowRight => (index + 1) % opened.len(),
            Key::Home => 0,
            Key::End => opened.len() - 1,
            _ => return,
        };
        let id = opened[next].resource.resource_id().to_owned();
        self.active.set(Some(id.clone()));
        crate::components::interaction::BrowserFocus::after_render(format!("resource-tab-{id}"));
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
    let tabs = ResourceTabs::use_tabs();
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let available = use_memo(move || sessions.catalog_available());
    let catalog = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        || async {
            DatasetClient::current()
                .project_resources()
                .await
                .map_err(|error| error.to_string())
        },
    );
    let resources = catalog.value();
    let panels = vec![
        Panel::new(
            "tables",
            "Tables",
            "tables",
            rsx! { super::resource_tree::ResourceTree { resources, tabs } },
        ),
        Panel::new(
            "datasets",
            "Workspace",
            "datasets",
            rsx! { ObjectWorkspace { resources, tabs } },
        ),
    ];
    let initial_layout = PanelLayout::new(LayoutNode::split(
        "data-columns",
        SplitAxis::Horizontal,
        0.25,
        LayoutNode::tile("tables", ["tables".to_owned()]),
        LayoutNode::tile("datasets", ["datasets".to_owned()]),
    ));
    rsx! { div { class: "data-workspace min-h-0 min-w-0 overflow-hidden",
        PanelWorkspace { panels, initial_layout, layout_scope: Some("spiral_robotics.panel-layout.v9.data".to_owned()) }
    } }
}

#[component]
fn ObjectWorkspace(
    resources: ReadSignal<Option<Result<Vec<ProjectResource>, String>>>,
    mut tabs: ResourceTabs,
) -> Element {
    let mut importing = tabs.importing;
    let mut imported_dataset = use_signal(|| None::<String>);
    use_effect(move || {
        let Some(name) = imported_dataset() else {
            return;
        };
        let resource = resources
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|resource| resource.selection().label() == name)
            })
            .cloned();
        if let Some(resource) = resource {
            imported_dataset.set(None);
            tabs.open(resource, None);
        }
    });
    let opened = tabs.opened.read().clone();
    let active = tabs.active_id();
    let choices = resources
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    rsx! { section { class: "data-object-workspace",
        header { class: "data-object-bar",
            div { class: "data-object-tabs", role: "tablist", "aria-label": "Open tables and datasets",
                for tab in opened.iter() {
                    {
                        let id = tab.resource.resource_id().to_owned();
                        let select_id = id.clone();
                        let close_id = id.clone();
                        let selected = active.as_deref() == Some(id.as_str()) && !importing();
                        rsx! { div { class: "data-object-tab", "data-active": selected,
                            button { id: "resource-tab-{id}", role: "tab", aria_selected: selected, aria_controls: "resource-panel-{id}", tabindex: if selected { "0" } else { "-1" },
                                title: tab.resource.selection().label(),
                                onclick: move |_| { tabs.active.set(Some(select_id.clone())); importing.set(false); },
                                onkeydown: move |event| { if matches!(event.key(), Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End) { event.prevent_default(); tabs.navigate(event.key()); } },
                                "{tab.resource.selection().path().last().unwrap()}"
                            }
                            button { class: "data-tab-close", "aria-label": "Close {tab.resource.selection().label()}", onclick: move |_| tabs.close(&close_id),
                                svg { view_box: "0 0 16 16", "aria-hidden": "true", path { d: "m4 4 8 8M12 4l-8 8" } }
                            }
                        } }
                    }
                }
            }
            label { class: "data-load", span { "Load" }
                select { "aria-label": "Load a table or dataset", value: active.clone().unwrap_or_default(), onchange: move |event| {
                    if let Some(resource) = choices.iter().find(|resource| resource.resource_id() == event.value()) { tabs.open(resource.clone(), None); importing.set(false); }
                }, option { value: "", selected: active.is_none(), "Choose…" }
                    for resource in resources.read().as_ref().and_then(|result| result.as_ref().ok()).into_iter().flatten() {
                        if matches!(resource.kind(), "table" | "view" | "dataset") { option { value: resource.resource_id(), selected: active.as_deref() == Some(resource.resource_id()), "{resource.selection().label()}" } }
                    }
                }
            }
            button { class: "data-import-action", aria_pressed: importing(), onclick: move |_| importing.toggle(), "Import" }
        }
        div { class: "data-session-content", hidden: !importing(), super::import_dataset::ImportDataset { on_done: move |name| { imported_dataset.set(Some(name)); importing.set(false); super::relations::RelationCatalogState::refresh(); }, on_close: move |_| importing.set(false) } }
        if opened.is_empty() && !importing() { div { class: "data-inspector-welcome", h2 { "Open your data" } p { "Choose a table or dataset from the catalog, or use Load to open it in a tab." } p { "Import MCAP recordings to create a native Spiral dataset." } } }
        for tab in opened {
            {
                let id = tab.resource.resource_id().to_owned();
                let visible = active.as_deref() == Some(id.as_str()) && !importing();
                rsx! { div { key: "{id}", id: "resource-panel-{id}", role: "tabpanel", "aria-labelledby": "resource-tab-{id}", class: "data-object-body", hidden: !visible,
                    ObjectSession { tab }
                } }
            }
        }
    } }
}

#[derive(Clone, Copy, PartialEq)]
enum ObjectSection {
    Inspect,
    Sql,
    Results,
}
#[component]
fn ObjectSession(tab: OpenResource) -> Element {
    let studio = crate::shell::state::StudioProjectState::current();
    let dataset = tab.resource.kind() == "dataset";
    let initial_sql = if dataset {
        tab.resource.selection().episodes_sql()
    } else {
        tab.resource.table_sql()
    };
    let controller = QueryController::use_controller(initial_sql);
    let mut section = use_signal(|| ObjectSection::Inspect);
    let revision = tab.revision;
    use_effect(use_reactive((&revision,), move |_| {
        section.set(ObjectSection::Inspect)
    }));
    use_effect(move || {
        if controller.query_running() {
            section.set(ObjectSection::Results);
        }
    });
    rsx! { div { class: "data-object-session",
        nav { class: "data-session-nav", "aria-label": "Object tools",
            for (value, label) in [(ObjectSection::Inspect, "Inspect"), (ObjectSection::Sql, "SQL"), (ObjectSection::Results, "Results")] {
                button { aria_pressed: section() == value, onclick: move |_| section.set(value), "{label}" }
            }
            span { title: tab.resource.selection().label(), "{tab.resource.selection().label()}" }
        }
        div { class: "data-session-content", hidden: section() != ObjectSection::Inspect,
            if dataset { NativeDatasetInspector { key: "{tab.revision}", selection: tab.resource.selection().clone(), topic: tab.topic } }
            else { TableInspector { resource: tab.resource.clone() } }
        }
        div { class: "data-session-content", hidden: section() != ObjectSection::Sql, QueryPanel { studio, controller } }
        div { class: "data-session-content", hidden: section() != ObjectSection::Results, QueryResultView { controller } }
    } }
}

#[component]
fn TableInspector(resource: ProjectResource) -> Element {
    let statement = resource.table_sql();
    let query = statement.clone();
    let mut preview = use_resource(move || {
        let query = query.clone();
        async move { DatasetClient::current().run_spiral_query(query, 100).await }
    });
    rsx! { section { class: "data-inspector", "aria-label": "Table inspector",
        match &*preview.read() {
            None => rsx! { p { class: "data-empty-copy", role: "status", "Loading table…" } },
            Some(Err(error)) => rsx! { div { class: "data-empty-copy", role: "alert", p { "{error}" } button { class: "data-text-action", onclick: move |_| preview.restart(), "Retry table" } } },
            Some(Ok(response)) => rsx! {
                details { class: "data-table-schema", summary { "Schema · {response.columns().len()} columns" }
                    ul { for column in response.columns() { li { code { "{column.name()}" } span { "{column.data_type()}" } } } }
                }
                {super::query_result::QueryExecutionPresentation::new(statement, 100, super::query_result::QueryState::Ready(response.clone())).render()}
            },
        }
    } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::native_datasets::NativeDatasetSelection;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone)]
    struct SessionHarness(Rc<RefCell<Option<(ResourceTabs, QueryController, QueryController)>>>);
    impl SessionHarness {
        fn render(self) -> Element {
            let tabs = ResourceTabs::use_tabs();
            let first = QueryController::use_controller("SELECT 1".into());
            let second = QueryController::use_controller("SELECT 2".into());
            *self.0.borrow_mut() = Some((tabs, first, second));
            VNode::empty()
        }
        fn resource(name: &str) -> ProjectResource {
            ProjectResource::new(
                NativeDatasetSelection::new(vec![name.to_owned()]).unwrap(),
                "dataset".into(),
                (name.into(), "v1".into()),
            )
        }
    }
    #[test]
    fn opening_outputs_reuses_dataset_tabs_without_sharing_sql_drafts() {
        let harness = SessionHarness(Rc::new(RefCell::new(None)));
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        let (tabs, first, second) = harness.0.borrow().unwrap();
        dom.in_runtime(|| {
            tabs.open(SessionHarness::resource("first"), None);
            first.set_sql("SELECT 7".into());
            tabs.open(SessionHarness::resource("second"), None);
            second.set_max_rows(25);
            tabs.open(SessionHarness::resource("first"), Some("/camera".into()));
            assert_eq!(tabs.opened.peek().len(), 2);
            assert_eq!(tabs.opened.peek()[0].topic.as_deref(), Some("/camera"));
            assert_eq!(first.sql(), "SELECT 7");
            assert_eq!(second.sql(), "SELECT 2");
            assert_eq!(first.max_rows(), 100);
            assert_eq!(second.max_rows(), 25);
            tabs.close("second");
            assert_eq!(tabs.active_id().as_deref(), Some("first"));
            tabs.close("first");
            assert!(tabs.active_id().is_none());
        });
    }
}
