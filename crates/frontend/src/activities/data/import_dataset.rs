//! Inspect a pinned MCAP source before publishing a dataset.
use super::DatasetClient;
use crate::format::ByteCountPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Fact, FactList, Field, Notice, NoticeTone,
    TextInput,
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
        // What is accepted is the repository field's to say: the title names
        // the files, the label where they come from.
        header { h2 { "Import MCAP dataset" } }
        if let Some(result) = receipt() {
            p { role: "status", "{result.message}" }
            footer {
                Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    onclick: move |_| { receipt.set(None); inspection.set(None); draft.name.set(String::new()); on_done.call(result.dataset.clone()); }, "Open dataset" }
            }
        } else {
            fieldset { disabled: busy(),
                Field { label: "Dataset name", TextInput { value: (draft.name)(), placeholder: "bench-recordings", oninput: move |value| { draft.name.set(value); inspection.set(None); } } }
                Field { label: "Public Hugging Face repository", TextInput { value: (draft.repository)(), placeholder: "owner/repository", oninput: move |value| { draft.repository.set(value); inspection.set(None); } } }
                Field { label: "Revision", TextInput { value: (draft.revision)(), oninput: move |value| { draft.revision.set(value); inspection.set(None); } } }
            }
            if let Some(error) = problem() { Notice { tone: NoticeTone::Danger, class: "data-import-error", "{error}" } }
            // What the source holds and what importing it creates, as
            // readings under the fields that found them.
            if let Some(detail) = inspection() {
                FactList { class: "data-import-review", mono: true,
                    Fact { label: "MCAP files", "{detail.files().len()}" }
                    Fact { label: "Size", "{detail.total_bytes().byte_label()}" }
                    Fact { label: "Revision", "{detail.resolved_revision()}" }
                    Fact { label: "Creates",
                        for name in detail.planned_relations() { span { class: "data-import-relation", "{name}" } }
                    }
                }
                for warning in detail.warnings() { Notice { tone: NoticeTone::Caution, class: "data-import-warning", "{warning}" } }
            }
            footer {
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    disabled: busy() || draft.name.read().trim().is_empty() || draft.repository.read().trim().is_empty(), onclick: move |_| {
                    if let Some(detail) = inspection() {
                        let source = HuggingFaceDatasetSource::new(detail.dataset_id(), detail.repository()).with_revision(detail.resolved_revision());
                        busy.set(true); problem.set(None);
                        spawn(async move {
                            let client = DatasetClient::current();
                            match client.import_dataset(source).await {
                                Ok(summary) => {
                                    let summary = match super::robot_discovery::RobotDiscoveryAgent::new(client).discover(&detail).await {
                                        Ok(()) => summary,
                                        Err(_) => {
                                            let mut warnings = summary.warnings().to_vec();
                                            warnings.push("Episodes imported, but the 3D view could not be prepared.".into());
                                            summary.with_warnings(warnings)
                                        }
                                    };
                                    receipt.set(Some(ImportReceipt::from(summary)));
                                    super::relations::CatalogRefresh::request();
                                },
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
                Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                    disabled: busy(), onclick: move |_| panel.request_close(), "Close" }
            }
        }
    } }
}
