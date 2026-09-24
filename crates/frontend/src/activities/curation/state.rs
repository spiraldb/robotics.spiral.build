//! Browser-owned curation: the loaded graph, its decoded payload,
//! node selection, episode focus, and saves. The controller is passed as a
//! prop like `WorkflowController`; every Curation view reads and edits through
//! it. Shared contract: extend it, never change what is here.
use super::document::{CURATION_ENCODING_VERSION, CurationGraph, DATASET_NODE_ID, SubtaskRow};
use super::population::{EpisodeSample, Population, PopulationReader};
use super::poster::PosterCache;
use crate::components::document_session::{
    DocumentCommand, DocumentSession, DocumentSummary, StudioDocument,
};
use crate::components::scoped_task::ScopedTask;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationDocument;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_ui::Place;
use std::collections::BTreeMap;

const UNTITLED: &str = "Untitled curation";

/// A save is refused while the document has another command in flight, so a
/// measurement's save waits for it rather than dropping what it measured.
const SAVE_WAIT_MS: u32 = 50;
const SAVE_WAIT_ROUNDS: usize = 200;

/// One quiet line per node that is measuring something right now.
pub(crate) type NodeActivity = BTreeMap<String, String>;

/// Where a node's long measurement stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunState {
    Idle,
    Running,
    /// Stop was pressed: nothing new starts, and what is in flight finishes
    /// and is saved.
    Stopping,
}

/// One long measurement the curation owns.
#[derive(Clone, Copy)]
struct OwnedRun {
    task: Task,
    wanted: bool,
}

/// What the curation's nodes are doing: the line each running node shows, and
/// the long runs the curation itself owns.
///
/// A run a workspace spawned was the workspace's, so choosing another node —
/// which unmounts it — ended an annotation part way and left its card saying
/// "Annotating 12 of 40 episodes" forever. Annotation and scoring therefore
/// run as the curation's own tasks and outlive the view that started them;
/// opening another document cancels them, since their node ids mean nothing
/// in it.
#[derive(Clone, Copy, PartialEq)]
struct RunBook {
    lines: Signal<NodeActivity>,
    owned: Signal<BTreeMap<String, OwnedRun>>,
}

impl RunBook {
    fn new() -> Self {
        Self {
            lines: Signal::new(NodeActivity::new()),
            owned: Signal::new(BTreeMap::new()),
        }
    }
    fn state(&self, node: &str) -> RunState {
        match self.owned.read().get(node) {
            None => RunState::Idle,
            Some(run) if run.wanted => RunState::Running,
            Some(_) => RunState::Stopping,
        }
    }
    fn wanted(&self, node: &str) -> bool {
        self.owned.peek().get(node).is_some_and(|run| run.wanted)
    }
    fn report(mut self, node: &str, line: Option<String>) {
        let mut lines = self.lines.write();
        match line {
            Some(line) => lines.insert(node.to_owned(), line),
            None => lines.remove(node),
        };
    }
    fn stop(mut self, node: &str) {
        if let Some(run) = self.owned.write().get_mut(node) {
            run.wanted = false;
        }
    }
    fn finish(mut self, node: &str) {
        self.owned.write().remove(node);
        self.report(node, None);
    }
    /// End every run at once, for a document that is being replaced.
    fn cancel_all(mut self) {
        for (_, run) in std::mem::take(&mut *self.owned.write()) {
            run.task.cancel();
        }
        self.lines.write().clear();
    }
    /// End the runs of nodes the graph no longer holds: a removed node's
    /// pass would otherwise go on paying for episodes it cannot write.
    fn cancel_removed(mut self, graph: &CurationGraph) {
        let removed = self
            .owned
            .peek()
            .keys()
            .filter(|node| graph.node(node).is_none())
            .cloned()
            .collect::<Vec<_>>();
        for node in removed {
            if let Some(run) = self.owned.write().remove(&node) {
                run.task.cancel();
            }
            self.report(&node, None);
        }
    }
}

/// What a running measurement holds of the curation: whether it is still
/// wanted, and where it says how far it has got.
#[derive(Clone)]
pub(super) struct RunHandle {
    node: String,
    book: RunBook,
}

impl RunHandle {
    /// The node the run measures for.
    pub(super) fn node(&self) -> &str {
        &self.node
    }
    /// Whether the operator still wants the run; Stop clears it, and the run
    /// starts nothing new.
    pub(super) fn wanted(&self) -> bool {
        self.book.wanted(&self.node)
    }
    /// The line the node's card and the top bar carry.
    pub(super) fn report(&self, line: String) {
        self.book.report(&self.node, Some(line));
    }
}

/// What a curation is doing right now, as the top bar reports it: one line
/// for each node that is running, and nothing at all at rest — what a graph
/// has measured is on its cards, and a resting total in the status line only
/// said it again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CurationRuns(Vec<String>);

impl CurationRuns {
    pub(crate) fn lines(&self) -> &[String] {
        &self.0
    }
}

impl StudioDocument for CurationDocument {
    const NOUN: &'static str = "curation";
    const COLLECTION: &'static str = "curations";
    const REPORTS_AS: MessageSource = MessageSource::Curation;

    fn id(&self) -> &str {
        self.id()
    }
    fn set_name(&mut self, name: String) {
        self.set_name(name);
    }
    fn fresh(id: String) -> Self {
        Self::new(id, UNTITLED, CURATION_ENCODING_VERSION, "{}").carrying(&CurationGraph::new(None))
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
trait CurationDraft {
    fn carrying(&self, payload: &CurationGraph) -> Self;
}

impl CurationDraft for CurationDocument {
    fn carrying(&self, payload: &CurationGraph) -> Self {
        CurationDocument::new(
            self.id(),
            self.name(),
            CURATION_ENCODING_VERSION,
            payload
                .encode()
                .unwrap_or_else(|_| self.payload().to_owned()),
        )
        .with_revision(self.revision())
    }
}

/// Where a curation asks the annotation view to look: one episode of one
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
pub(crate) struct CurationController {
    session: DocumentSession<CurationDocument>,
    /// The graph as the views read and edit it. Every accepted edit is encoded
    /// back into the session's draft, so "Unsaved changes" is one comparison.
    payload: Signal<CurationGraph>,
    selected_node: Signal<Option<String>>,
    focus: Signal<Option<EpisodeFocus>>,
    /// What every node is doing right now. A run in flight is not in the
    /// payload, so it says so here and the canvas and the top bar read it: a
    /// node the operator has moved away from still shows that it is working.
    runs: RunBook,
    /// Where on screen the last Find similar was asked from, until the search
    /// it opened has carried its query card in from there.
    launch: Signal<Option<Place>>,
    /// The episodes every node is interpreted over, read once per dataset.
    sample: Resource<Result<Option<EpisodeSample>, String>>,
}

impl CurationController {
    pub(crate) fn use_controller() -> Self {
        // A subtask's poster is the same picture whichever of the curation's
        // nodes is selected, so the cache the backfilled ones are cut into
        // belongs to the curation, not to the view that first asked. It is
        // provided here, beside the payload, and read by the poster frames
        // themselves; nothing in it ever reaches the saved document.
        use_context_provider(PosterCache::new);
        let session = DocumentSession::use_session();
        let payload = use_signal(|| CurationGraph::new(None));
        // Only a new dataset reads a new sample; editing the graph never does.
        let dataset = use_memo(move || payload.read().dataset().cloned());
        let sample = use_resource(move || async move {
            match dataset() {
                Some(dataset) => EpisodeSample::load(&dataset).await.map(Some),
                None => Ok(None),
            }
        });
        use_hook(|| Self {
            session,
            payload,
            selected_node: Signal::new(Some(DATASET_NODE_ID.to_owned())),
            focus: Signal::new(None),
            runs: RunBook::new(),
            launch: Signal::new(None),
            sample,
        })
    }

    /// The episodes this curation is interpreted over: `None` while they are
    /// read, and for a graph without a dataset. A resource keeps its last
    /// value while it reads the next, so the sample of the dataset before —
    /// another curation's, just after switching — is held back rather than
    /// listed as this one's.
    pub(super) fn sample(&self) -> Option<EpisodeSample> {
        let sample = self
            .sample
            .read()
            .as_ref()
            .and_then(|loaded| loaded.as_ref().ok())
            .cloned()
            .flatten()?;
        let current = self.with_payload(|payload| payload.dataset() == Some(sample.dataset()));
        current.then_some(sample)
    }
    /// Why the sample could not be read, when it could not.
    pub(super) fn sample_error(&self) -> Option<String> {
        self.sample
            .read()
            .as_ref()
            .and_then(|loaded| loaded.as_ref().err())
            .cloned()
    }
    pub(super) fn reload_sample(mut self) {
        self.sample.restart();
    }
    /// What `node` hands on, and what reaches it, once the sample is read.
    pub(super) fn populations(&self, node: &str) -> Option<(Population, Population)> {
        let sample = self.sample()?;
        self.with_payload(|payload| {
            let reader = PopulationReader::new(payload, &sample);
            Some((reader.input(node), reader.of(node)))
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
    /// What every running node is doing, which is all the top bar wants
    /// from a curation.
    pub(crate) fn runs(&self) -> CurationRuns {
        CurationRuns(self.runs.lines.read().values().cloned().collect())
    }

    /// Read the payload without cloning it.
    pub(crate) fn with_payload<R>(&self, read: impl FnOnce(&CurationGraph) -> R) -> R {
        read(&self.payload.read())
    }
    /// Apply one edit. An edit that fails, or leaves the graph invalid, is
    /// rolled back and its message returned. A removed selected node falls
    /// back to the dataset node.
    pub(crate) fn edit<R>(
        mut self,
        edit: impl FnOnce(&mut CurationGraph) -> Result<R, String>,
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
            self.runs.cancel_removed(&self.payload.peek());
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
    pub(crate) fn activity(&self) -> NodeActivity {
        self.runs.lines.read().clone()
    }
    /// One node's line while a view's own work runs; `None` once it stops.
    pub(crate) fn report_activity(self, node: &str, line: Option<String>) {
        self.runs.report(node, line);
    }
    /// Start `run` for `node` as the curation's own task, so it keeps going
    /// while the operator looks at other nodes. A node already running is
    /// left alone.
    pub(super) fn start_run<F>(self, node: &str, run: impl FnOnce(RunHandle) -> F)
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        let mut owned = self.runs.owned;
        if owned.peek().contains_key(node) {
            return;
        }
        let book = self.runs;
        let handle = RunHandle {
            node: node.to_owned(),
            book,
        };
        let finished = node.to_owned();
        let work = run(handle);
        let task = ScopedTask::spawn(self.payload.origin_scope(), async move {
            work.await;
            book.finish(&finished);
        });
        owned
            .write()
            .insert(node.to_owned(), OwnedRun { task, wanted: true });
    }
    /// Where `node`'s own run stands.
    pub(super) fn run_state(&self, node: &str) -> RunState {
        self.runs.state(node)
    }
    /// Ask `node`'s run to start nothing new.
    pub(super) fn stop_run(self, node: &str) {
        self.runs.stop(node);
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
    /// Ask the annotation's Find similar node about one of its rows and show
    /// that node, which runs the question as soon as it reads it. `from` is
    /// where the row was on screen, so the question can be seen arriving.
    pub(crate) fn find_similar(mut self, annotation: &str, row: &SubtaskRow, from: Option<Place>) {
        if let Ok(node) = self.edit(|payload| payload.search_for(annotation, row)) {
            self.launch.set(from);
            self.selected_node.set(Some(node));
        }
    }
    /// Where the last Find similar was asked from, once.
    pub(crate) fn take_launch(mut self) -> Option<Place> {
        self.launch.write().take()
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
    /// Curation opens on a blank graph, so the first read lists what is saved
    /// without opening any of it.
    pub(crate) fn refresh_documents(self) {
        self.session.refresh_summaries();
    }

    /// Start a new, unsaved graph over `dataset`.
    pub(crate) fn create(self, name: impl Into<String>, dataset: Option<DatasetSelection>) {
        let name = name.into();
        let mut document = DocumentSession::<CurationDocument>::fresh();
        if !name.trim().is_empty() {
            document.set_name(name);
        }
        self.session.install(
            document.carrying(&CurationGraph::new(dataset)),
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
    fn adopt(mut self, document: &CurationDocument) -> Result<(), String> {
        let graph = Self::graph_of(document)?;
        self.runs.cancel_all();
        self.payload.set(graph);
        self.selected_node.set(Some(DATASET_NODE_ID.to_owned()));
        self.focus.set(None);
        Ok(())
    }
    fn graph_of(document: &CurationDocument) -> Result<CurationGraph, String> {
        CurationGraph::decode(document.encoding(), document.payload())
    }
    /// Save the current payload at the loaded revision. Edits made while the
    /// save is in flight stay local and unsaved.
    pub(crate) fn save(self, studio: StudioProjectState) {
        self.session.save(studio);
    }
    /// Save once the document has no other command in flight, so what a
    /// measurement just wrote reaches Studio even while a save is running.
    pub(super) async fn save_when_idle(self, studio: StudioProjectState) {
        for _ in 0..SAVE_WAIT_ROUNDS {
            if self.command().is_none() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(SAVE_WAIT_MS).await;
        }
        self.save(studio);
    }
    /// Drop local edits and return to the last saved or loaded state. The
    /// selection only moves when the node it named is no longer there.
    pub(crate) fn discard(mut self) {
        self.session.discard(|baseline| {
            self.payload.set(Self::graph_of(baseline)?);
            self.runs.cancel_removed(&self.payload.peek());
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
