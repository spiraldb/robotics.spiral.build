//! Project resources; dataset browsing begins with its episode population.
use super::screen::ResourceTabs;
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
            button { class: "data-tree-refresh", "aria-label": "Refresh catalog", title: "Refresh catalog", onclick: move |_| super::relations::RelationCatalogState::refresh(),
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
                        ul { class: "data-tree-list", for resource in roots { ResourceBranch { key: "{resource.resource_id()}", resource, resources, tabs } } }
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
    let expandable = !children.is_empty();
    let selectable = matches!(resource.kind(), "table" | "view" | "dataset");
    let selected = tabs.active_id().as_deref() == Some(resource.resource_id());
    let open = resource.clone();
    rsx! { li { class: "data-tree-branch",
        div { class: "data-tree-row", "data-selected": selected,
            if expandable { button { class: "data-tree-toggle", aria_expanded: expanded(), "aria-label": "Expand {resource.selection().label()}", onclick: move |_| expanded.toggle(),
                svg { view_box: "0 0 16 16", "aria-hidden": "true", style: if expanded() { "transform: rotate(90deg)" } else { "" }, path { d: "m6 4 4 4-4 4" } }
            } } else { span { class: "data-tree-indent" } }
            button { class: "data-tree-name", aria_pressed: selected, title: "{resource.selection().label()} · {resource.kind()}", onclick: move |_| { if selectable { tabs.open(open.clone()); } else { expanded.toggle(); } },
                ResourceIcon { dataset }
                span { "{path.last().unwrap()}" }
                small { "{resource.kind()}" }
            }
        }
        if expanded() {
            ul { class: "data-tree-list data-tree-children",
                for child in children { ResourceBranch { key: "{child.resource_id()}", resource: child, resources, tabs } }
            }
        }
    } }
}

#[component]
fn ResourceIcon(dataset: bool) -> Element {
    rsx! { svg { class: "data-resource-icon", view_box: "0 0 16 16", "aria-hidden": "true",
        if dataset { ellipse { cx: "8", cy: "4", rx: "5", ry: "2" } path { d: "M3 4v8c0 2.7 10 2.7 10 0V4M3 8c0 2.7 10 2.7 10 0" } }
        else { rect { x: "2.5", y: "2.5", width: "11", height: "11", rx: "1" } path { d: "M2.5 6h11M6 6v7.5" } }
    } }
}
