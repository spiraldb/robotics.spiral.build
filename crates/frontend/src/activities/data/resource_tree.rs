//! Project data and expandable native outputs, with Studio internals folded away.
use super::{
    DatasetClient,
    screen::{OpenResource, ResourceTabs},
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::ProjectResource;

type ResourceCatalog = ReadSignal<Option<Result<Vec<ProjectResource>, String>>>;
#[component]
pub(super) fn ResourceTree(resources: ResourceCatalog, tabs: ResourceTabs) -> Element {
    let mut search = use_signal(String::new);
    let result = resources.read();
    rsx! { section { class: "data-resource-tree", "aria-label": "SpiralDB catalog",
        div { class: "data-catalog-toolbar",
            input { class: "field", r#type: "search", placeholder: "Find tables or datasets…", "aria-label": "Find tables or datasets", value: search(), oninput: move |event| search.set(event.value()) }
            button { class: "data-tree-refresh", "aria-label": "Refresh catalog", title: "Refresh catalog", onclick: move |_| super::relations::CatalogRefresh::request(),
                crate::components::interaction::RefreshIcon {}
            }
        }
        div { class: "data-tree-scroll",
            match &*result {
                None => rsx! { p { class: "data-empty-copy", role: "status", "Reading SpiralDB catalog…" } },
                Some(Err(error)) => rsx! { p { class: "data-empty-copy", role: "alert", "{error} Refresh to retry." } },
                Some(Ok(entries)) if entries.is_empty() => rsx! { p { class: "data-empty-copy", "This project has no tables or datasets yet. Use Import to add MCAP recordings." } },
                Some(Ok(entries)) => {
                    let filter = search().to_lowercase();
                    let roots = entries.iter().filter(|resource| if filter.is_empty() { resource.selection().path().len() == 1 } else { resource.selection().label().to_lowercase().contains(&filter) }).cloned().collect::<Vec<_>>();
                    rsx! {
                        if roots.is_empty() { p { class: "data-empty-copy", "No matching tables or datasets." } }
                        ul { class: "data-tree-list", for resource in roots.iter().filter(|resource| !resource.is_studio_internal()) {
                            ResourceBranch { key: "{resource.resource_id()}", resource: resource.clone(), resources, tabs }
                        } }
                        if roots.iter().any(ProjectResource::is_studio_internal) {
                            details { class: "data-studio-resources", open: !filter.is_empty(),
                                summary { "Studio internals" }
                                ul { class: "data-tree-list", for resource in roots.iter().filter(|resource| resource.is_studio_internal()) {
                                    ResourceBranch { key: "{resource.resource_id()}", resource: resource.clone(), resources, tabs }
                                } }
                            }
                        }
                    }
                },
            }
        }
    } }
}

#[component]
fn ResourceBranch(
    resource: ProjectResource,
    resources: ResourceCatalog,
    tabs: ResourceTabs,
) -> Element {
    let mut expanded = use_signal(|| false);
    let path = resource.selection().path();
    let children = resources
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .into_iter()
        .flatten()
        .filter(|child| {
            child.selection().path().len() == path.len() + 1
                && child.selection().path().starts_with(path)
        })
        .cloned()
        .collect::<Vec<_>>();
    let dataset = resource.kind() == "dataset";
    let expandable = dataset || !children.is_empty();
    let selectable = matches!(resource.kind(), "table" | "view" | "dataset");
    let selected = tabs.active_id().as_deref() == Some(resource.resource_id());
    let resource_id = resource.resource_id().to_owned();
    use_effect(move || {
        if dataset && tabs.active_id().as_deref() == Some(resource_id.as_str()) {
            expanded.set(true);
        }
    });
    let open = resource.clone();
    rsx! { li { class: "data-tree-branch",
        div { class: "data-tree-row", "data-selected": selected,
            if expandable { button { class: "data-tree-toggle", aria_expanded: expanded(), "aria-label": if expanded() { format!("Collapse {}", resource.selection().label()) } else { format!("Expand {}", resource.selection().label()) }, onclick: move |_| expanded.toggle(),
                svg { view_box: "0 0 16 16", "aria-hidden": "true", style: if expanded() { "transform: rotate(90deg)" } else { "" }, path { d: "m6 4 4 4-4 4" } }
            } } else { span { class: "data-tree-indent" } }
            button { class: "data-tree-name", aria_pressed: selected, title: "{resource.selection().label()} · {resource.kind()}", onclick: move |_| { if selectable { tabs.open(open.clone()); if dataset { expanded.set(true); } } else { expanded.toggle(); } },
                ResourceIcon { dataset }
                span { "{path.last().unwrap()}" }
                small { "{resource.kind()}" }
            }
        }
        if expanded() {
            ul { class: "data-tree-list data-tree-children",
                if dataset { DatasetOutputs { key: "{resource.dataset().unwrap().version_id()}", resource: resource.clone(), tabs } }
                for child in children { ResourceBranch { key: "{child.resource_id()}", resource: child, resources, tabs } }
            }
        }
    } }
}

#[component]
fn DatasetOutputs(resource: ProjectResource, tabs: ResourceTabs) -> Element {
    let requested = resource.selection().clone();
    let available = use_memo(move || {
        crate::shell::project::ProjectSessionSignals::current().catalog_available()
    });
    let mut inspection = crate::client::queries::QueryPolicy::on_demand(
        crate::client::queries::QueryDomain::Catalog,
    )
    .use_when(
        move || *available.read(),
        move || DatasetClient::current().inspect_dataset(requested.clone()),
    );
    let response = inspection.read();
    rsx! {
        if !*available.read() {
            li { class: "data-tree-status", "Connect to SpiralDB to browse outputs." }
        } else if *inspection.state().read() == UseResourceState::Pending {
            li { class: "data-tree-status", role: "status", "Loading outputs…" }
        } else { match response.as_ref() {
            Some(Ok(detail)) => {
                // Native schema variants can share a topic; the topic reader handles them together.
                let topics = detail.topics().iter().map(|topic| topic.name()).collect::<std::collections::BTreeSet<_>>();
                rsx! {
                    if detail.members().is_some() {
                        li { class: "data-tree-row",
                            span { class: "data-tree-indent" }
                            button { class: "data-tree-name", onclick: { let resource = resource.clone(); move |_| tabs.open(resource.clone()) },
                                ResourceIcon { dataset: false }
                                span { "Episodes" }
                            }
                        }
                    }
                    for topic in topics {
                        li { class: "data-tree-row", "data-selected": tabs.active_id().as_deref() == Some(OpenResource::topic_id(&resource, topic).as_str()),
                            span { class: "data-tree-indent" }
                            button { class: "data-tree-name", title: topic,
                                onclick: { let resource = resource.clone(); let topic = topic.to_owned(); move |_| tabs.open_topic(resource.clone(), topic.clone()) },
                                ResourceIcon { dataset: false }
                                span { "{topic}" }
                            }
                        }
                    }
                    if detail.members().is_none() && detail.topics().is_empty() {
                        li { class: "data-tree-status", "No dataset outputs yet." }
                    }
                }
            },
            Some(Err(error)) => rsx! { li { class: "data-tree-status", role: "alert",
                p { "{error}" }
                button { class: "data-text-action", onclick: move |_| inspection.restart(), "Retry outputs" }
            } },
            None => VNode::empty(),
        } }
    }
}

#[component]
fn ResourceIcon(dataset: bool) -> Element {
    rsx! { svg { class: "data-resource-icon", view_box: "0 0 16 16", "aria-hidden": "true",
        if dataset { ellipse { cx: "8", cy: "4", rx: "5", ry: "2" } path { d: "M3 4v8c0 2.7 10 2.7 10 0V4M3 8c0 2.7 10 2.7 10 0" } }
        else { rect { x: "2.5", y: "2.5", width: "11", height: "11", rx: "1" } path { d: "M2.5 6h11M6 6v7.5" } }
    } }
}
