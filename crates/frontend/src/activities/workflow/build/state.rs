//! Browser-owned Build document: the loaded graph, its decoded payload,
//! node selection, episode focus, and saves. The controller is passed as a
//! prop like `WorkflowController`; every Build view reads and edits through
//! it. Shared contract: extend it, never change what is here.
use super::client::BuildClient;
use super::document::{BUILD_ENCODING_VERSION, BuildPayload, DATASET_NODE_ID};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::build::{BuildDocument, BuildDocumentSummary};
use spiral_robotics_common::command_failure::CommandFailureCode;
use spiral_robotics_common::datasets::DatasetSelection;

const UNTITLED: &str = "Untitled build";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildPendingCommand {
    Listing,
    Opening,
    Saving,
}

/// Where an exploration asks the annotation view to look: one episode of one
/// annotation node, and optionally one interval within it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeFocus {
    node: String,
    episode: String,
    interval: Option<(f64, f64)>,
}
impl EpisodeFocus {
    pub(crate) fn new(
        node: impl Into<String>,
        episode: impl Into<String>,
        interval: Option<(f64, f64)>,
    ) -> Self {
        Self {
            node: node.into(),
            episode: episode.into(),
            interval,
        }
    }
    pub(crate) fn node(&self) -> &str {
        &self.node
    }
    pub(crate) fn episode(&self) -> &str {
        &self.episode
    }
    pub(crate) fn interval(&self) -> Option<(f64, f64)> {
        self.interval
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct BuildController {
    /// Identity, name, and the revision the payload was loaded at.
    document: Signal<BuildDocument>,
    /// The name as last saved, for unsaved-change detection.
    saved_name: Signal<String>,
    payload: Signal<BuildPayload>,
    /// The payload as last saved or loaded.
    baseline: Signal<BuildPayload>,
    documents: Signal<Vec<BuildDocumentSummary>>,
    selected_node: Signal<Option<String>>,
    focus: Signal<Option<EpisodeFocus>>,
    command: Signal<Option<BuildPendingCommand>>,
    conflict: Signal<bool>,
    error: Signal<Option<String>>,
}

impl BuildController {
    pub(crate) fn use_controller() -> Self {
        use_hook(|| {
            let payload = BuildPayload::new(None);
            Self {
                document: Signal::new(Self::fresh_document(UNTITLED, &payload)),
                saved_name: Signal::new(UNTITLED.to_owned()),
                payload: Signal::new(payload.clone()),
                baseline: Signal::new(payload),
                documents: Signal::new(Vec::new()),
                selected_node: Signal::new(Some(DATASET_NODE_ID.to_owned())),
                focus: Signal::new(None),
                command: Signal::new(None),
                conflict: Signal::new(false),
                error: Signal::new(None),
            }
        })
    }
    fn fresh_document(name: &str, payload: &BuildPayload) -> BuildDocument {
        BuildDocument::new(
            format!(
                "build-{}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * u64::MAX as f64) as u64
            ),
            name,
            BUILD_ENCODING_VERSION,
            payload.encode().unwrap_or_else(|_| "{}".to_owned()),
        )
    }
    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.payload.origin_scope(), task);
    }

    pub(crate) fn id(&self) -> String {
        self.document.read().id().to_owned()
    }
    pub(crate) fn name(&self) -> String {
        self.document.read().name().to_owned()
    }
    pub(crate) fn set_name(mut self, name: String) {
        self.document.write().set_name(name);
    }

    /// Read the payload without cloning it.
    pub(crate) fn with_payload<R>(&self, read: impl FnOnce(&BuildPayload) -> R) -> R {
        read(&self.payload.read())
    }
    /// A snapshot of the payload, for views that keep their own copy.
    pub(crate) fn payload(&self) -> BuildPayload {
        self.payload.read().clone()
    }
    /// Apply one edit. An edit that fails, or leaves the graph invalid, is
    /// rolled back and its message returned. A removed selected node falls
    /// back to the dataset node.
    pub(crate) fn edit<R>(
        mut self,
        edit: impl FnOnce(&mut BuildPayload) -> Result<R, String>,
    ) -> Result<R, String> {
        let outcome = {
            let mut payload = self.payload.write();
            let previous = payload.clone();
            match edit(&mut payload).and_then(|value| payload.validate().map(|()| value)) {
                Ok(value) => Ok(value),
                Err(error) => {
                    *payload = previous;
                    Err(error)
                }
            }
        };
        if outcome.is_ok() {
            let selection_gone = self
                .selected_node
                .peek()
                .as_deref()
                .is_some_and(|id| self.payload.peek().node(id).is_none());
            if selection_gone {
                self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
            }
        }
        outcome
    }
    pub(crate) fn unsaved_changes(&self) -> bool {
        *self.payload.read() != *self.baseline.read()
            || self.document.read().name() != self.saved_name.read().as_str()
    }

    pub(crate) fn selected_node(&self) -> Option<String> {
        self.selected_node.read().clone()
    }
    pub(crate) fn select_node(mut self, node: Option<String>) {
        self.selected_node.set(node);
    }
    pub(crate) fn focus(&self) -> Option<EpisodeFocus> {
        self.focus.read().clone()
    }
    /// Select the focus's annotation node and ask its view to show the
    /// episode. The view calls `take_focus` once it has done so.
    pub(crate) fn focus_episode(mut self, focus: EpisodeFocus) {
        self.selected_node.set(Some(focus.node.clone()));
        self.focus.set(Some(focus));
    }
    pub(crate) fn take_focus(mut self) -> Option<EpisodeFocus> {
        self.focus.write().take()
    }

    pub(crate) fn command(&self) -> Option<BuildPendingCommand> {
        *self.command.read()
    }
    pub(crate) fn conflict(&self) -> bool {
        *self.conflict.read()
    }
    /// The last load or list failure, cleared by the next success.
    pub(crate) fn error(&self) -> Option<String> {
        self.error.read().clone()
    }
    pub(crate) fn documents(&self) -> Vec<BuildDocumentSummary> {
        self.documents.read().clone()
    }

    pub(crate) fn refresh_documents(mut self) {
        if self.command().is_some() {
            return;
        }
        self.command.set(Some(BuildPendingCommand::Listing));
        self.spawn(async move {
            self.list_documents().await;
            self.command.set(None);
        });
    }
    async fn list_documents(mut self) {
        match BuildClient::current().list().await {
            Ok(documents) => {
                self.documents.set(documents);
                self.error.set(None);
            }
            Err(error) => self.error.set(Some(error.to_string())),
        }
    }
    /// Start a new, unsaved graph over `dataset`.
    pub(crate) fn create(self, name: impl Into<String>, dataset: Option<DatasetSelection>) {
        let name = name.into();
        let name = if name.trim().is_empty() {
            UNTITLED.to_owned()
        } else {
            name
        };
        let payload = BuildPayload::new(dataset);
        let document = Self::fresh_document(&name, &payload);
        self.install(document, payload);
    }
    pub(crate) fn open(mut self, id: String) {
        if self.command().is_some() {
            return;
        }
        self.command.set(Some(BuildPendingCommand::Opening));
        self.spawn(async move {
            match BuildClient::current().load(&id).await {
                Ok(document) => match BuildPayload::decode(document.encoding(), document.payload())
                {
                    Ok(payload) => {
                        self.install(document, payload);
                        self.error.set(None);
                    }
                    Err(error) => self.error.set(Some(error)),
                },
                Err(error) => self.error.set(Some(error.to_string())),
            }
            self.command.set(None);
        });
    }
    fn install(mut self, document: BuildDocument, payload: BuildPayload) {
        self.saved_name.set(document.name().to_owned());
        self.document.set(document);
        self.baseline.set(payload.clone());
        self.payload.set(payload);
        self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
        self.focus.set(None);
        self.conflict.set(false);
    }
    /// Save the current payload at the loaded revision. Edits made while the
    /// save is in flight stay local and unsaved.
    pub(crate) fn save(mut self, studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let payload = self.payload();
        let encoded = match payload.encode() {
            Ok(encoded) => encoded,
            Err(error) => {
                studio.report_from(MessageSource::Workflow, error);
                return;
            }
        };
        let current = self.document.read().clone();
        let submitted = BuildDocument::new(
            current.id(),
            current.name(),
            BUILD_ENCODING_VERSION,
            encoded,
        )
        .with_revision(current.revision());
        if let Err(error) = submitted.validate() {
            studio.report_from(MessageSource::Workflow, error);
            return;
        }
        self.command.set(Some(BuildPendingCommand::Saving));
        self.spawn(async move {
            match BuildClient::current().save(&submitted).await {
                Ok(saved) => {
                    if self.document.peek().id() == saved.id() {
                        self.document.write().set_revision(saved.revision());
                        self.saved_name.set(saved.name().to_owned());
                        self.baseline.set(payload);
                        self.conflict.set(false);
                    }
                    self.list_documents().await;
                }
                Err(error) => {
                    self.conflict
                        .set(error.code() == CommandFailureCode::Conflict);
                    studio.report_from(MessageSource::Workflow, error.to_string());
                }
            }
            self.command.set(None);
        });
    }
    /// Drop local edits and return to the last saved or loaded state.
    pub(crate) fn discard(mut self) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.baseline.peek().clone();
        let name = self.saved_name.peek().clone();
        self.payload.set(baseline);
        self.document.write().set_name(name);
        let selection_gone = self
            .selected_node
            .peek()
            .as_deref()
            .is_some_and(|id| self.payload.peek().node(id).is_none());
        if selection_gone {
            self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
        }
    }
}
