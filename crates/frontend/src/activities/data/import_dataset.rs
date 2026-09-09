//! Inspect a pinned MCAP source before publishing a native dataset.
use super::DatasetClient;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};

#[derive(Clone, Copy, PartialEq)]
struct ImportDraft {
    name: Signal<String>,
    repository: Signal<String>,
    revision: Signal<String>,
}
impl ImportDraft {
    fn source(self) -> HuggingFaceDatasetSource {
        HuggingFaceDatasetSource::new(self.name.read().trim(), self.repository.read().trim())
            .with_revision(self.revision.read().trim())
    }
}
#[derive(Clone)]
struct ImportReceipt {
    dataset: String,
    message: String,
}
impl From<HuggingFaceDatasetImportSummary> for ImportReceipt {
    fn from(summary: HuggingFaceDatasetImportSummary) -> Self {
        Self {
            dataset: summary.relations().first().cloned().unwrap_or_default(),
            message: format!(
                "Imported {} MCAP files. {}",
                summary.files(),
                summary.warnings().join(" ")
            ),
        }
    }
}
#[component]
pub(super) fn ImportDataset(on_done: EventHandler<String>) -> Element {
    let panel = dioxus_workbench::PanelContext::current();
    let mut draft = ImportDraft {
        name: use_signal(String::new),
        repository: use_signal(String::new),
        revision: use_signal(|| "main".to_owned()),
    };
    let mut inspection = use_signal(|| None::<HuggingFaceDatasetInspection>);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut receipt = use_signal(|| None::<ImportReceipt>);
    rsx! { section { class: "data-import-pane", "aria-label": "Import MCAP dataset",
        header { h2 { "Import MCAP dataset" } p { "Create a native Spiral dataset from MCAP files in a public Hugging Face repository." } }
        if let Some(result) = receipt() {
            p { role: "status", "{result.message}" }
            button { class: "data-import-action", onclick: move |_| { receipt.set(None); inspection.set(None); draft.name.set(String::new()); on_done.call(result.dataset.clone()); }, "Open dataset" }
        } else {
            fieldset { disabled: busy(),
                label { span { "Dataset name" } input { class: "field", value: (draft.name)(), placeholder: "bench-recordings", oninput: move |event| { draft.name.set(event.value()); inspection.set(None); } } }
                label { span { "Hugging Face repository" } input { class: "field", value: (draft.repository)(), placeholder: "owner/repository", oninput: move |event| { draft.repository.set(event.value()); inspection.set(None); } } }
                label { span { "Revision" } input { class: "field", value: (draft.revision)(), oninput: move |event| { draft.revision.set(event.value()); inspection.set(None); } } }
            }
            if let Some(error) = problem() { p { class: "data-import-error", role: "alert", "{error}" } }
            if let Some(detail) = inspection() {
                div { class: "data-import-review",
                    strong { "{detail.files().len()} MCAP files · {detail.total_bytes()} bytes" }
                    p { "Revision: {detail.resolved_revision()}" }
                    p { "Spiral will create:" }
                    ul { for name in detail.planned_relations() { li { code { "{name}" } } } }
                    for warning in detail.warnings() { p { "{warning}" } }
                }
            }
            footer {
                button { class: "data-import-action", disabled: busy() || draft.name.read().trim().is_empty() || draft.repository.read().trim().is_empty(), onclick: move |_| {
                    if let Some(detail) = inspection() {
                        let source = HuggingFaceDatasetSource::new(detail.dataset_id(), detail.repository()).with_revision(detail.resolved_revision());
                        busy.set(true); problem.set(None);
                        spawn(async move {
                            match DatasetClient::current().import_dataset(source).await {
                                Ok(summary) => { receipt.set(Some(ImportReceipt::from(summary))); super::relations::RelationCatalogState::refresh(); },
                                Err(error) => {
                                    let message = error.to_string();
                                    let uncertain = error.code() == spiral_robotics_common::command_failure::CommandFailureCode::Transport || message.contains("uncertain");
                                    problem.set(Some(if uncertain { format!("{message} Check the catalog before retrying; the request may have committed.") } else { message }));
                                    inspection.set(None);
                                },
                            }
                            busy.set(false);
                        });
                    } else {
                        busy.set(true); problem.set(None);
                        spawn(async move {
                            match DatasetClient::current().inspect_import(draft.source()).await { Ok(detail) => inspection.set(Some(detail)), Err(error) => problem.set(Some(error.to_string())) }
                            busy.set(false);
                        });
                    }
                }, if busy() { "Working…" } else if inspection.read().is_some() { "Import dataset" } else { "Inspect source" } }
                button { class: "data-text-action", disabled: busy(), onclick: move |_| panel.request_close(), "Close" }
            }
        }
    } }
}
