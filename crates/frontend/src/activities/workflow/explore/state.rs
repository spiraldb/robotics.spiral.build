//! Browser-owned exploration: the loaded graph, its decoded payload,
//! node selection, episode focus, and saves. The controller is passed as a
//! prop like `WorkflowController`; every Explore view reads and edits through
//! it. Shared contract: extend it, never change what is here.
use super::document::{
    DATASET_NODE_ID, EXPLORATION_ENCODING_VERSION, ExplorationNodeKind, ExplorationPayload,
};
use super::poster::PosterCache;
use super::review::Counted;
use crate::components::document_session::{
    DocumentCommand, DocumentSession, DocumentSummary, StudioDocument,
};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_common::exploration::ExplorationDocument;
use std::collections::BTreeMap;

const UNTITLED: &str = "Untitled exploration";

/// One quiet line per node that is measuring something right now.
pub(crate) type ExplorationActivity = BTreeMap<String, String>;

/// What an exploration's annotation nodes have measured so far, as the top
/// bar reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExplorationMeasure {
    episodes: usize,
    subtasks: usize,
}

impl ExplorationMeasure {
    fn of(payload: &ExplorationPayload) -> Self {
        let mut episodes = 0;
        let mut subtasks = 0;
        for node in payload.nodes() {
            if let ExplorationNodeKind::BreakDownSubtasks(annotation) = node.kind() {
                episodes += annotation.completed_episodes().len();
                subtasks += payload.subtasks_for(node.id()).count();
            }
        }
        Self { episodes, subtasks }
    }
    pub(crate) fn has_measured(&self) -> bool {
        self.episodes > 0
    }
    /// The measurement state in the operator's words. A graph that has
    /// measured nothing says so rather than counting zeroes.
    pub(crate) fn measured(&self) -> String {
        if self.episodes == 0 {
            return "Nothing measured yet".to_owned();
        }
        format!(
            "{} annotated · {}",
            Counted::episodes(self.episodes),
            Counted::subtasks(self.subtasks),
        )
    }
}

impl StudioDocument for ExplorationDocument {
    const NOUN: &'static str = "exploration";
    const COLLECTION: &'static str = "explorations";
    const REPORTS_AS: MessageSource = MessageSource::Workflow;

    fn id(&self) -> &str {
        self.id()
    }
    fn set_name(&mut self, name: String) {
        self.set_name(name);
    }
    fn fresh(id: String) -> Self {
        Self::new(id, UNTITLED, EXPLORATION_ENCODING_VERSION, "{}")
            .carrying(&ExplorationPayload::new(None))
    }
    fn acknowledge_save(&mut self, saved: &Self) {
        if self.id() == saved.id() {
            self.set_revision(saved.revision());
        }
    }
}

/// The graph travels inside the document as opaque text. Replacing that text
/// keeps the document's identity, name, and revision, so one encoded payload
/// is all that separates a draft from what Studio stored.
trait ExplorationDraft {
    fn carrying(&self, payload: &ExplorationPayload) -> Self;
}

impl ExplorationDraft for ExplorationDocument {
    fn carrying(&self, payload: &ExplorationPayload) -> Self {
        ExplorationDocument::new(
            self.id(),
            self.name(),
            EXPLORATION_ENCODING_VERSION,
            payload
                .encode()
                .unwrap_or_else(|_| self.payload().to_owned()),
        )
        .with_revision(self.revision())
    }
}

/// Where an exploration asks the annotation view to look: one episode of one
/// annotation node, and optionally one interval within it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeFocus {
    node: String,
    episode: EpisodeId,
    interval: Option<(f64, f64)>,
}
impl EpisodeFocus {
    pub(crate) fn new(
        node: impl Into<String>,
        episode: EpisodeId,
        interval: Option<(f64, f64)>,
    ) -> Self {
        Self {
            node: node.into(),
            episode,
            interval,
        }
    }
    pub(crate) fn node(&self) -> &str {
        &self.node
    }
    pub(crate) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(crate) fn interval(&self) -> Option<(f64, f64)> {
        self.interval
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ExplorationController {
    session: DocumentSession<ExplorationDocument>,
    /// The graph as the views read and edit it. Every accepted edit is encoded
    /// back into the session's draft, so "Unsaved changes" is one comparison.
    payload: Signal<ExplorationPayload>,
    selected_node: Signal<Option<String>>,
    focus: Signal<Option<EpisodeFocus>>,
    /// One line per node that is measuring something right now. A run in
    /// flight is not in the payload, so the view that runs it says so here and
    /// the canvas reads it: a node the operator has scrolled away from still
    /// shows that it is working.
    activity: Signal<ExplorationActivity>,
}

impl ExplorationController {
    pub(crate) fn use_controller() -> Self {
        // A subtask's poster is the same picture whichever of the exploration's
        // nodes is selected, so the cache the backfilled ones are cut into
        // belongs to the exploration, not to the view that first asked. It is
        // provided here, beside the payload, and read by the poster frames
        // themselves; nothing in it ever reaches the saved document.
        use_context_provider(PosterCache::new);
        let session = DocumentSession::use_session();
        use_hook(|| Self {
            session,
            payload: Signal::new(ExplorationPayload::new(None)),
            selected_node: Signal::new(Some(DATASET_NODE_ID.to_owned())),
            focus: Signal::new(None),
            activity: Signal::new(ExplorationActivity::new()),
        })
    }

    pub(crate) fn id(&self) -> String {
        self.session.with_draft(|document| document.id().to_owned())
    }
    pub(crate) fn name(&self) -> String {
        self.session
            .with_draft(|document| document.name().to_owned())
    }
    pub(crate) fn set_name(self, name: String) {
        self.session.set_name(name);
    }
    /// How much of its dataset this exploration has measured, which is all
    /// the top bar wants from it.
    pub(crate) fn measure(&self) -> ExplorationMeasure {
        self.with_payload(ExplorationMeasure::of)
    }

    /// Read the payload without cloning it.
    pub(crate) fn with_payload<R>(&self, read: impl FnOnce(&ExplorationPayload) -> R) -> R {
        read(&self.payload.read())
    }
    /// Apply one edit. An edit that fails, or leaves the graph invalid, is
    /// rolled back and its message returned. A removed selected node falls
    /// back to the dataset node.
    pub(crate) fn edit<R>(
        mut self,
        edit: impl FnOnce(&mut ExplorationPayload) -> Result<R, String>,
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
            self.record_payload();
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
    /// Carry the edited graph into the draft document, which is what a save
    /// sends and what "Unsaved changes" is measured against.
    fn record_payload(self) {
        let payload = self.payload.peek().clone();
        self.session.edit(|document| {
            let carried = document.carrying(&payload);
            *document = carried;
        });
    }
    pub(crate) fn unsaved_changes(&self) -> bool {
        self.session.unsaved_changes()
    }

    pub(crate) fn selected_node(&self) -> Option<String> {
        self.selected_node.read().clone()
    }
    pub(crate) fn select_node(mut self, node: Option<String>) {
        self.selected_node.set(node);
    }
    /// What every node that is measuring right now has to say for itself.
    pub(crate) fn activity(&self) -> ExplorationActivity {
        self.activity.read().clone()
    }
    /// One node's line while it runs; `None` once it has stopped.
    pub(crate) fn report_activity(mut self, node: &str, line: Option<String>) {
        let mut activity = self.activity.write();
        match line {
            Some(line) => activity.insert(node.to_owned(), line),
            None => activity.remove(node),
        };
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

    pub(crate) fn command(&self) -> Option<DocumentCommand> {
        self.session.command()
    }
    /// The last load or list failure, cleared by the next success.
    pub(crate) fn error(&self) -> Option<String> {
        self.session.load_error()
    }
    pub(crate) fn documents(&self) -> Vec<DocumentSummary> {
        self.session.summaries()
    }
    /// Explore opens on a blank graph, so the first read lists what is saved
    /// without opening any of it.
    pub(crate) fn refresh_documents(self) {
        self.session.refresh_summaries();
    }

    /// Start a new, unsaved graph over `dataset`.
    pub(crate) fn create(self, name: impl Into<String>, dataset: Option<DatasetSelection>) {
        let name = name.into();
        let mut document = DocumentSession::<ExplorationDocument>::fresh();
        if !name.trim().is_empty() {
            document.set_name(name);
        }
        self.session.install(
            document.carrying(&ExplorationPayload::new(dataset)),
            move |opened| self.adopt(opened),
        );
    }
    pub(crate) fn open(self, id: String) {
        if self.session.command().is_some() {
            return;
        }
        self.session
            .open(Some(id), move |opened| self.adopt(opened));
    }
    /// Take the document's graph as the one on screen. A payload this Studio
    /// cannot read refuses the document rather than emptying the canvas.
    fn adopt(mut self, document: &ExplorationDocument) -> Result<(), String> {
        self.payload.set(Self::graph_of(document)?);
        self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
        self.focus.set(None);
        Ok(())
    }
    fn graph_of(document: &ExplorationDocument) -> Result<ExplorationPayload, String> {
        ExplorationPayload::decode(document.encoding(), document.payload())
    }
    /// Save the current payload at the loaded revision. Edits made while the
    /// save is in flight stay local and unsaved.
    pub(crate) fn save(self, studio: StudioProjectState) {
        self.session.save(studio);
    }
    /// Drop local edits and return to the last saved or loaded state. The
    /// selection only moves when the node it named is no longer there.
    pub(crate) fn discard(mut self) {
        self.session.discard(|baseline| {
            self.payload.set(Self::graph_of(baseline)?);
            let selection_gone = self
                .selected_node
                .peek()
                .as_deref()
                .is_some_and(|id| self.payload.peek().node(id).is_none());
            if selection_gone {
                self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
            }
            Ok(())
        });
    }
}
