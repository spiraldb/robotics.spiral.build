//! Project catalog beside independent object sessions.
use super::{
    DatasetClient, datasets::DatasetInspector, query_result::QueryResultView,
    state::QueryController, workbench::QueryPanel,
};
use crate::shell::frame::WorkspaceFrame;
use dioxus::prelude::*;
use dioxus_workbench::{
    GroupContext, LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis,
};
use spiral_robotics_common::datasets::ProjectResource;

#[derive(Clone, PartialEq)]
pub(super) struct OpenResource {
    resource: ProjectResource,
    revision: u64,
    topic: Option<String>,
}
impl OpenResource {
    fn new(resource: ProjectResource, topic: Option<String>) -> Self {
        Self {
            resource,
            revision: 0,
            topic,
        }
    }
    fn id(&self) -> String {
        self.topic.as_ref().map_or_else(
            || self.resource.resource_id().to_owned(),
            |topic| Self::topic_id(&self.resource, topic),
        )
    }
    pub(super) fn topic_id(resource: &ProjectResource, topic: &str) -> String {
        format!("dataset-output:{}:{topic}", resource.resource_id())
    }
    fn title(&self) -> String {
        self.topic
            .clone()
            .unwrap_or_else(|| self.resource.selection().path().last().unwrap().clone())
    }
    fn sql(&self) -> String {
        match &self.topic {
            Some(topic) => self.resource.selection().topic_sql(topic),
            None if self.resource.kind() == "dataset" => self.resource.selection().episodes_sql(),
            None => self.resource.table_sql(),
        }
    }
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
    pub(super) fn open(self, resource: ProjectResource) {
        self.open_session(OpenResource::new(resource, None));
    }
    pub(super) fn open_topic(self, resource: ProjectResource, topic: String) {
        self.open_session(OpenResource::new(resource, Some(topic)));
    }
    fn open_session(mut self, session: OpenResource) {
        let id = session.id();
        let mut opened = self.opened.write();
        if let Some(tab) = opened.iter_mut().find(|tab| tab.id() == id) {
            if tab.resource != session.resource {
                tab.resource = session.resource;
                tab.revision = tab.revision.wrapping_add(1);
            }
        } else {
            opened.push(session);
        }
        drop(opened);
        self.active.set(Some(id));
    }
    // Registry removal approves closing; workbench owns selection and focus.
    fn close(mut self, id: &str) {
        if id == "data-import" {
            self.importing.set(false);
        } else {
            self.opened.write().retain(|tab| tab.id() != id);
        }
        if self.active.peek().as_deref() == Some(id) {
            self.active.set(None);
        }
    }
    pub(super) fn active_id(self) -> Option<String> {
        self.active.read().clone()
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
    let mut tabs = ResourceTabs::use_tabs();
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
            tabs.close("data-import");
            tabs.open(resource);
        }
    });
    let mut panels = vec![Panel::new(
        "tables",
        "Catalog",
        "tables",
        rsx! { super::resource_tree::ResourceTree { resources, tabs } },
    )];
    for tab in tabs.opened.read().iter() {
        panels.push(
            Panel::new(
                tab.id(),
                tab.title(),
                "datasets",
                rsx! { ObjectSession { tab: tab.clone() } },
            )
            .with_closable(true)
            .with_class("data-object-panel"),
        );
    }
    if (tabs.importing)() {
        panels.push(Panel::new("data-import", "Import", "datasets", rsx! {
            super::import_dataset::ImportDataset {
                on_done: move |name| { imported_dataset.set(Some(name)); super::relations::CatalogRefresh::request(); },
            }
        }).with_closable(true).with_class("data-object-panel"));
    }
    if tabs.opened.read().is_empty() && !(tabs.importing)() {
        panels.push(Panel::new("data-open", "Open data", "datasets", rsx! {
            div { class: "data-inspector-welcome", h2 { "Open your data" }
                p { "Choose a table or dataset from the catalog, or use Load to open it in a tab." }
                p { "Import MCAP recordings to create a Spiral dataset." }
            }
        }).with_class("data-object-panel"));
    }
    let initial_layout = PanelLayout::new(LayoutNode::split(
        "data-columns",
        SplitAxis::Horizontal,
        0.18,
        LayoutNode::tile("tables", ["tables".to_owned()]),
        LayoutNode::empty_tile("datasets"),
    ));
    let active_panel = tabs.active_id().map(PanelId::new);
    rsx! { div { class: "data-workspace min-h-0 min-w-0 overflow-hidden",
        PanelWorkspace { panels, initial_layout, active_panel,
            layout_scope: Some("spiral_robotics.panel-layout.v11.data".to_owned()),
            on_panel_activate: move |id: PanelId| tabs.active.set(Some(id.as_str().to_owned())),
            on_panel_close: move |panel: PanelId| tabs.close(panel.as_str()),
            group_toolbar: move |group: GroupContext| {
                if group.active_panel().is_some_and(|id| id.as_str() != "tables") {
                    rsx! { ObjectActions { resources, tabs, active: group.active_panel().map(|id| id.as_str().to_owned()) } }
                } else { VNode::empty() }
            },
        }
    } }
}

#[component]
fn ObjectActions(
    resources: ReadSignal<Option<Result<Vec<ProjectResource>, String>>>,
    mut tabs: ResourceTabs,
    active: Option<String>,
) -> Element {
    let mut importing = tabs.importing;
    let choices = resources
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned()
        .unwrap_or_default();
    let active = active.filter(|id| choices.iter().any(|resource| resource.resource_id() == id));
    rsx! { div { class: "data-panel-actions", role: "group", "aria-label": "Load or import data",
            label { class: "data-load", span { "Load" }
                select { "aria-label": "Load a table or dataset", value: active.clone().unwrap_or_default(), onchange: move |event| {
                    if let Some(resource) = choices.iter().find(|resource| resource.resource_id() == event.value()) { tabs.open(resource.clone()); }
                }, option { value: "", selected: active.is_none(), "Choose…" }
                    for resource in resources.read().as_ref().and_then(|result| result.as_ref().ok()).into_iter().flatten() {
                        if matches!(resource.kind(), "table" | "view" | "dataset") { option { value: resource.resource_id(), selected: active.as_deref() == Some(resource.resource_id()), "{resource.selection().label()}" } }
                    }
                }
            }
            button { class: "data-import-action", onclick: move |_| { importing.set(true); tabs.active.set(Some("data-import".to_owned())); }, "Import" }
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
    let dataset = tab.resource.kind() == "dataset" && tab.topic.is_none();
    let controller = QueryController::use_controller(tab.sql());
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
            if dataset { DatasetInspector { key: "{tab.revision}", selection: tab.resource.selection().clone() } }
            else { RelationPreview { key: "{tab.revision}", statement: tab.sql() } }
        }
        div { class: "data-session-content", hidden: section() != ObjectSection::Sql, QueryPanel { studio, controller } }
        div { class: "data-session-content", hidden: section() != ObjectSection::Results, QueryResultView { controller } }
    } }
}

#[component]
fn RelationPreview(statement: String) -> Element {
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
    use spiral_robotics_common::datasets::DatasetSelection;
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
                DatasetSelection::new(vec![name.to_owned()]).unwrap(),
                "dataset".into(),
                (name.into(), "v1".into()),
            )
        }
    }
    #[test]
    fn opening_datasets_reuses_tabs_without_sharing_sql_drafts() {
        let harness = SessionHarness(Rc::new(RefCell::new(None)));
        let mut dom = VirtualDom::new_with_props(SessionHarness::render, harness.clone());
        dom.rebuild_in_place();
        let (mut tabs, first, second) = harness.0.borrow().unwrap();
        dom.in_runtime(|| {
            tabs.open(SessionHarness::resource("first"));
            first.set_sql("SELECT 7".into());
            tabs.open(SessionHarness::resource("second"));
            second.set_max_rows(25);
            tabs.open(SessionHarness::resource("first"));
            assert_eq!(tabs.opened.peek().len(), 2);
            assert_eq!(tabs.opened.peek()[0].revision, 0);
            tabs.open(ProjectResource::new(
                DatasetSelection::new(vec!["first".into()]).unwrap(),
                "dataset".into(),
                ("first".into(), "v2".into()),
            ));
            assert_eq!(tabs.opened.peek()[0].revision, 1);
            assert_eq!(first.sql(), "SELECT 7");
            assert_eq!(second.sql(), "SELECT 2");
            assert_eq!(first.max_rows(), 100);
            assert_eq!(second.max_rows(), 25);
            let resource = SessionHarness::resource("first");
            let topic = "/camera/front";
            let topic_id = OpenResource::topic_id(&resource, topic);
            tabs.open_topic(resource.clone(), topic.into());
            tabs.open_topic(resource.clone(), topic.into());
            assert_eq!(tabs.opened.peek().len(), 3);
            assert_eq!(tabs.active_id().as_deref(), Some(topic_id.as_str()));
            assert_eq!(
                tabs.opened.peek()[2].sql(),
                resource.selection().topic_sql(topic)
            );
            tabs.close(&topic_id);
            assert_eq!(tabs.opened.peek().len(), 2);
            tabs.open(resource);
            tabs.importing.set(true);
            tabs.close("data-import");
            assert_eq!(tabs.active_id().as_deref(), Some("first"));
            assert!(!*tabs.importing.peek());
            tabs.close("second");
            assert_eq!(tabs.active_id().as_deref(), Some("first"));
            tabs.close("first");
            assert!(tabs.active_id().is_none());
        });
    }
}
