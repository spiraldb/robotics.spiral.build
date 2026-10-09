//! Import a dataset from a public Hugging Face repository: MCAP recordings,
//! each file one episode, or a LeRobot v3 dataset, each episode a member.
//! Either is inspected first, pinned to the commit its revision resolves to,
//! and then imported; a LeRobot import is a run the backend holds while
//! Spiral copies every episode, polled here for its phase.
use super::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::format::ByteCountPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    LeRobotImportPhase, LeRobotImportStatus,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Fact, FactList, Field, ModeButton, ModeGroup,
    Notice, NoticeTone, TextInput,
};

/// How often a running LeRobot import is asked where it has got.
const IMPORT_POLL_MS: u32 = 2_000;

/// What a repository holds, and so how it is imported.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ImportFormat {
    /// `.mcap` files, each one episode.
    McapRecordings,
    /// A LeRobot v3 dataset, which Spiral reads through a Location.
    LeRobot,
}

impl ImportFormat {
    const ALL: [Self; 2] = [Self::McapRecordings, Self::LeRobot];
    fn label(self) -> &'static str {
        match self {
            Self::McapRecordings => "MCAP recordings",
            Self::LeRobot => "Hugging Face (LeRobot)",
        }
    }
    /// What an inspection counts.
    fn files_label(self) -> &'static str {
        match self {
            Self::McapRecordings => "MCAP files",
            Self::LeRobot => "Files",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ImportDraft {
    format: Signal<ImportFormat>,
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
        format: use_signal(|| ImportFormat::McapRecordings),
        name: use_signal(String::new),
        repository: use_signal(String::new),
        revision: use_signal(|| "main".to_owned()),
    };
    let mut inspection = use_signal(|| None::<HuggingFaceDatasetInspection>);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut receipt = use_signal(|| None::<ImportReceipt>);
    let mut run = use_signal(|| None::<LeRobotImportStatus>);
    let format = (draft.format)();
    if let Some(status) = run() {
        return rsx! {
            LeRobotImportProgress { status, on_done, on_close: move |()| run.set(None) }
        };
    }
    rsx! { section { class: "data-import-pane", "aria-label": "Import dataset",
        header { h2 { "Import dataset" } }
        if let Some(result) = receipt() {
            p { role: "status", "{result.message}" }
            footer {
                Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    onclick: move |_| { receipt.set(None); inspection.set(None); draft.name.set(String::new()); on_done.call(result.dataset.clone()); }, "Open dataset" }
            }
        } else {
            ModeGroup { label: "What the repository holds", class: "data-import-format",
                for each in ImportFormat::ALL {
                    ModeButton { key: "{each.label()}", purpose: ActionPurpose::Select, checked: each == format,
                        disabled: busy(),
                        onclick: move |_| { draft.format.set(each); inspection.set(None); problem.set(None); },
                        "{each.label()}" }
                }
            }
            fieldset { disabled: busy(),
                Field { label: "Dataset name", TextInput { value: (draft.name)(), placeholder: "bench-recordings", oninput: move |value| { draft.name.set(value); inspection.set(None); } } }
                Field { label: "Public Hugging Face repository", TextInput { value: (draft.repository)(), placeholder: "owner/repository", oninput: move |value| { draft.repository.set(value); inspection.set(None); } } }
                Field { label: "Revision", TextInput { value: (draft.revision)(), placeholder: "a commit hash pins the import", oninput: move |value| { draft.revision.set(value); inspection.set(None); } } }
            }
            if let Some(error) = problem() { Notice { tone: NoticeTone::Danger, class: "data-import-error", "{error}" } }
            // What the source holds and what importing it creates, as
            // readings under the fields that found them.
            if let Some(detail) = inspection() {
                FactList { class: "data-import-review", mono: true,
                    Fact { label: format.files_label(), "{detail.files().len()}" }
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
                    let client = DatasetClient::current();
                    busy.set(true); problem.set(None);
                    let Some(detail) = inspection() else {
                        spawn(async move {
                            let inspected = match format {
                                ImportFormat::McapRecordings => client.inspect_import(draft.source()).await,
                                ImportFormat::LeRobot => client.inspect_lerobot(&draft.source()).await,
                            };
                            match inspected { Ok(detail) => inspection.set(Some(detail)), Err(error) => problem.set(Some(error.to_string())) }
                            busy.set(false);
                        });
                        return;
                    };
                    let source = HuggingFaceDatasetSource::new(detail.dataset_id(), detail.repository()).with_revision(detail.resolved_revision());
                    spawn(async move {
                        match format {
                            ImportFormat::McapRecordings => match client.import_dataset(source).await {
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
                            },
                            ImportFormat::LeRobot => match client.start_lerobot_import(&source).await {
                                Ok(status) => { inspection.set(None); run.set(Some(status)); }
                                Err(error) => { problem.set(Some(error.to_string())); inspection.set(None); }
                            },
                        }
                        busy.set(false);
                    });
                }, if busy() { "Working…" } else if inspection.read().is_some() { "Import dataset" } else { "Inspect source" } }
                Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                    disabled: busy(), onclick: move |_| panel.request_close(), "Close" }
            }
        }
    } }
}

/// A LeRobot import the backend is running, polled until it ends: its phase
/// while Spiral lists the repository and copies every episode, then the way
/// to the new dataset, or Spiral's error as it said it.
#[component]
fn LeRobotImportProgress(
    status: LeRobotImportStatus,
    on_done: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut latest = use_signal(|| status.clone());
    let id = status.id().to_owned();
    let polled = QueryPolicy::new(QueryDomain::Catalog, IMPORT_POLL_MS).use_when(
        move || !latest.peek().phase().is_settled(),
        move || DatasetClient::current().lerobot_import(&id),
    );
    use_effect(move || {
        if let Some(Ok(status)) = &*polled.read() {
            if status.phase() == &LeRobotImportPhase::Done
                && latest.peek().phase() != &LeRobotImportPhase::Done
            {
                super::relations::CatalogRefresh::request();
            }
            latest.set(status.clone());
        }
    });
    let status = latest();
    let source = status.source();
    let dataset = format!("dataset_{}", source.dataset_id());
    let poll_error = match &*polled.read() {
        Some(Err(error)) => Some(error.to_string()),
        _ => None,
    };
    rsx! { section { class: "data-import-pane", "aria-label": "Import dataset",
        header { h2 { "Importing {source.dataset_id()}" } }
        FactList { class: "data-import-review", mono: true,
            Fact { label: "Repository", "{source.repository()}" }
            Fact { label: "Revision", "{source.revision()}" }
        }
        match status.phase() {
            LeRobotImportPhase::Done => rsx! {
                p { role: "status", "{status.phase().label()}" }
                footer {
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                        onclick: move |_| on_done.call(dataset.clone()), "Open dataset" }
                }
            },
            LeRobotImportPhase::Failed { error } => rsx! {
                Notice { tone: NoticeTone::Danger, class: "data-import-error", "{error}" }
                footer {
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                        onclick: move |_| on_close.call(()), "Back" }
                }
            },
            phase => rsx! {
                p { role: "status", "aria-live": "polite", "{phase.label()}…" }
                if let Some(error) = poll_error {
                    Notice { tone: NoticeTone::Caution, class: "data-import-warning", "{error}" }
                }
            },
        }
    } }
}
