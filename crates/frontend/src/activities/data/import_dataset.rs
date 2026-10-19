//! Import a dataset from a public Hugging Face repository: MCAP recordings,
//! each file one episode, or a LeRobot v3 dataset, each of its episodes one.
//! Either is inspected first, pinned to the commit its revision resolves to,
//! and then imported by a run the backend holds while it writes every
//! episode, polled here for how far it has got.
use super::DatasetClient;
use super::robot_discovery::RobotDiscoveryAgent;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::format::ByteCountPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetFormat, DatasetImportPhase, DatasetImportRequest, DatasetImportStatus, DatasetSelection,
    HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Fact, FactList, Field, ModeButton, ModeGroup,
    Notice, NoticeTone, TextInput,
};

/// How often a running import is asked where it has got.
const IMPORT_POLL_MS: u32 = 2_000;

/// What the Import screen calls what a repository holds.
trait ImportFormatPresentation: Sized {
    const ALL: [Self; 2];
    fn label(self) -> &'static str;
    /// What an inspection counts.
    fn files_label(self) -> &'static str;
}

impl ImportFormatPresentation for DatasetFormat {
    const ALL: [Self; 2] = [Self::Mcap, Self::LeRobot];
    fn label(self) -> &'static str {
        match self {
            Self::Mcap => "MCAP recordings",
            Self::LeRobot => "Hugging Face (LeRobot)",
        }
    }
    fn files_label(self) -> &'static str {
        match self {
            Self::Mcap => "MCAP files",
            Self::LeRobot => "Files",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ImportDraft {
    format: Signal<DatasetFormat>,
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

/// An import the backend has started: the dataset it creates, what
/// inspecting its repository found, and the run's status.
#[derive(Clone, PartialEq)]
struct StartedImport {
    dataset: DatasetSelection,
    inspection: HuggingFaceDatasetInspection,
    status: DatasetImportStatus,
}

#[component]
pub(super) fn ImportDataset(on_done: EventHandler<DatasetSelection>) -> Element {
    let panel = dioxus_workbench::PanelContext::current();
    let mut draft = ImportDraft {
        format: use_signal(|| DatasetFormat::Mcap),
        name: use_signal(String::new),
        repository: use_signal(String::new),
        revision: use_signal(|| "main".to_owned()),
    };
    let mut inspection = use_signal(|| None::<HuggingFaceDatasetInspection>);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut run = use_signal(|| None::<StartedImport>);
    let format = (draft.format)();
    if let Some(started) = run() {
        return rsx! {
            ImportProgress { started, on_done, on_close: move |()| run.set(None) }
        };
    }
    rsx! { section { class: "data-import-pane", "aria-label": "Import dataset",
        header { h2 { "Import dataset" } }
        ModeGroup { label: "What the repository holds", class: "data-import-format",
            for each in DatasetFormat::ALL {
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
                            DatasetFormat::Mcap => client.inspect_import(draft.source()).await,
                            DatasetFormat::LeRobot => client.inspect_lerobot(&draft.source()).await,
                        };
                        match inspected { Ok(detail) => inspection.set(Some(detail)), Err(error) => problem.set(Some(error.to_string())) }
                        busy.set(false);
                    });
                    return;
                };
                let dataset = match DatasetSelection::new(detail.dataset_id()) {
                    Ok(dataset) => dataset,
                    Err(error) => { problem.set(Some(error)); busy.set(false); return; }
                };
                let source = HuggingFaceDatasetSource::new(detail.dataset_id(), detail.repository()).with_revision(detail.resolved_revision());
                spawn(async move {
                    match client.start_import(&DatasetImportRequest::new(format, source)).await {
                        Ok(status) => run.set(Some(StartedImport { dataset, inspection: detail, status })),
                        Err(error) => {
                            let message = error.to_string();
                            let uncertain = error.code() == spiral_robotics_common::command_failure::CommandFailureCode::Transport || message.contains("uncertain");
                            problem.set(Some(if uncertain { format!("{message} Check the catalog before retrying; the import may have started.") } else { message }));
                        },
                    }
                    inspection.set(None);
                    busy.set(false);
                });
            }, if busy() { "Working…" } else if inspection.read().is_some() { "Import dataset" } else { "Inspect source" } }
            Button { purpose: ActionPurpose::Close, variant: ButtonVariant::Link, size: ButtonSize::Compact,
                disabled: busy(), onclick: move |_| panel.request_close(), "Close" }
        }
    } }
}

/// How far preparing an imported MCAP dataset's 3D view has got: the
/// browser reads the robot its repository describes once every episode is
/// written.
#[derive(Clone, Copy, PartialEq)]
enum RobotPreparation {
    Preparing,
    Prepared,
    Failed,
}

/// An import the backend is running, polled until it ends: its phase while
/// it reads the repository and writes every episode, then the way to the new
/// dataset, or the run's error as it said it.
#[component]
fn ImportProgress(
    started: StartedImport,
    on_done: EventHandler<DatasetSelection>,
    on_close: EventHandler<()>,
) -> Element {
    let mut latest = use_signal(|| started.status.clone());
    let mut robot = use_signal(|| None::<RobotPreparation>);
    let id = started.status.id().to_owned();
    let polled = QueryPolicy::new(QueryDomain::Catalog, IMPORT_POLL_MS).use_when(
        move || !latest.peek().phase().is_settled(),
        move || DatasetClient::current().import_status(&id),
    );
    let inspection = started.inspection.clone();
    use_effect(move || {
        let Some(Ok(status)) = &*polled.read() else {
            return;
        };
        let finished = status.phase() == &DatasetImportPhase::Done
            && latest.peek().phase() != &DatasetImportPhase::Done;
        latest.set(status.clone());
        if !finished {
            return;
        }
        super::relations::CatalogRefresh::request();
        if status.request().format() == DatasetFormat::Mcap {
            robot.set(Some(RobotPreparation::Preparing));
            let inspection = inspection.clone();
            spawn(async move {
                let prepared = RobotDiscoveryAgent::new(DatasetClient::current())
                    .discover(&inspection)
                    .await;
                robot.set(Some(if prepared.is_ok() {
                    RobotPreparation::Prepared
                } else {
                    RobotPreparation::Failed
                }));
            });
        }
    });
    let status = latest();
    let source = status.request().source();
    let dataset = started.dataset.clone();
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
            DatasetImportPhase::Done if robot() == Some(RobotPreparation::Preparing) => rsx! {
                p { role: "status", "aria-live": "polite", "Preparing the 3D view…" }
            },
            DatasetImportPhase::Done => rsx! {
                p { role: "status", "{status.phase().label()}" }
                if robot() == Some(RobotPreparation::Failed) {
                    Notice { tone: NoticeTone::Caution, class: "data-import-warning",
                        "Episodes imported, but the 3D view could not be prepared." }
                }
                footer {
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                        onclick: move |_| on_done.call(dataset.clone()), "Open dataset" }
                }
            },
            DatasetImportPhase::Failed { error } => rsx! {
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
