//! Dataset navigation extends the existing light desktop workbench.
//! Selection reveals native topics; preview actions populate the shared Results panel.
use crate::activities::data::DatasetClient;
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use crate::shell::{project::ProjectSessionSignals, state::studio};
use dioxus::prelude::*;
use spiral_robotics_common::native_datasets::NativeDatasetSelection;

#[component]
pub(crate) fn NativeDatasetsPanel() -> Element {
    let studio = studio();
    let sessions = ProjectSessionSignals::current();
    let mut selected = use_signal(|| None::<NativeDatasetSelection>);
    let mut catalog = use_resource(move || async move {
        if !sessions.storage_available() {
            return Ok(Vec::new());
        }
        DatasetClient::current().native_datasets().await
    });
    let mut inspection = use_resource(move || {
        let selection = selected.read().clone();
        async move {
            match selection {
                Some(selection) => DatasetClient::current()
                    .inspect_native_dataset(selection)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
    });
    let datasets = catalog.read().clone();
    let detail = inspection.read().clone();
    let inspecting = *inspection.state().read() == UseResourceState::Pending;
    let query_available = sessions.catalog_available();
    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface text-ink", "aria-label": "Spiral datasets",
            header { class: "flex min-h-10 shrink-0 items-center gap-2 border-b border-line px-3",
                h2 { class: "min-w-0 flex-1 text-xs font-medium", "Datasets" }
                Action { purpose: ActionPurpose::Refresh, variant: ActionVariant::Outline, disabled: !sessions.storage_available(), onclick: move |_| { catalog.restart(); inspection.restart(); }, "Refresh" }
            }
            div { class: "min-h-0 flex-1 overflow-auto",
                if !sessions.storage_available() {
                    p { class: "p-3 text-xs text-ink-soft", "Connect to SpiralDB to browse datasets." }
                } else {
                    match datasets {
                        None => rsx! { p { class: "p-3 text-xs text-ink-soft", role: "status", "Loading datasets…" } },
                        Some(Err(error)) => rsx! { p { class: "p-3 text-xs text-danger", role: "alert", "{error}" } },
                        Some(Ok(datasets)) if datasets.is_empty() => rsx! { p { class: "p-3 text-xs text-ink-soft", "No Spiral datasets yet. Import MCAP recordings to create one." } },
                        Some(Ok(datasets)) => rsx! {
                            ul { class: "border-b border-line", "aria-label": "Available datasets",
                                for dataset in datasets {
                                    {
                                        let selection = dataset.selection().clone();
                                        let label = selection.label();
                                        let active = selected.read().as_ref() == Some(&selection);
                                        rsx! {
                                            li { key: "{dataset.resource_id()}",
                                                button { class: if active { "w-full bg-selected px-3 py-2 text-left text-xs text-accent" } else { "w-full px-3 py-2 text-left text-xs hover:bg-hover focus-visible:outline-2 focus-visible:outline-accent" },
                                                    "aria-pressed": active,
                                                    onclick: move |_| { studio.selected_dataset().set(selection.label()); selected.set(Some(selection.clone())); },
                                                    span { class: "block break-words font-medium", "{label}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                    if inspecting && selected.read().is_some() {
                        p { class: "p-3 text-xs text-ink-soft", role: "status", "Loading dataset details…" }
                    } else {
                        match detail {
                            Some(Err(error)) => rsx! { p { class: "p-3 text-xs text-danger", role: "alert", "{error}" } },
                            Some(Ok(Some(inspection))) => {
                                let selection = inspection.dataset().selection().clone();
                                let episode_sql = selection.episodes_sql();
                                let can_preview = query_available && selection.can_preview();
                                rsx! {
                                    div { class: "space-y-2 border-b border-line p-3",
                                        h3 { class: "break-words text-sm font-medium", "{selection.label()}" }
                                        if let Some(members) = inspection.members() {
                                            p { class: "text-xs text-ink-soft", "{members} episodes · {inspection.topics().len()} topics" }
                                            Action { purpose: ActionPurpose::Select, variant: ActionVariant::Outline, disabled: !can_preview, onclick: move |_| {
                                                let controller = studio.data_controller();
                                                controller.set_sql(episode_sql.clone());
                                                controller.run_query(studio, query_available);
                                            }, "Preview episodes" }
                                        } else {
                                            p { class: "text-xs text-ink-soft", "This dataset has no native episode catalog to preview." }
                                        }
                                    }
                                    if !inspection.topics().is_empty() {
                                        details { class: "border-b border-line text-xs",
                                            summary { class: "cursor-pointer px-3 py-2 font-medium", "All dataset topics" }
                                            p { class: "px-3 pb-2 text-[11px] text-ink-soft", "Previews include all episodes in this dataset." }
                                            ul { class: "divide-y divide-line", "aria-label": "All dataset topics",
                                                for topic in inspection.topics() {
                                                    {
                                                        let sql = selection.topic_sql(topic.name());
                                                        rsx! {
                                                            li { key: "{topic.name()}",
                                                                button { class: "w-full px-3 py-2 text-left hover:bg-hover focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50", disabled: !can_preview,
                                                                    onclick: move |_| { let controller = studio.data_controller(); controller.set_sql(sql.clone()); controller.run_query(studio, query_available); },
                                                                    span { class: "block break-all font-medium", "{topic.name()}" }
                                                                    span { class: "mt-1 block text-[11px] text-ink-soft", {topic.schemas().join(", ")} }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if inspection.members().is_some() && can_preview {
                                        super::native_episodes::NativeEpisodeBrowser { key: "{inspection.dataset().resource_id()}-{inspection.dataset().version_id()}", inspection: inspection.clone() }
                                    }

                                }
                            },
                            _ => rsx! { p { class: "p-3 text-xs text-ink-soft", "Choose a dataset to inspect its episodes and topics." } },
                        }
                    }
                }
            }
        }
    }
}
