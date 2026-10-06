//! Browser-owned curation: the loaded graph, its decoded payload, what the
//! preview has read over the sample, node selection, episode focus, and
//! saves. The controller is passed as a prop; every Curation view reads and
//! edits through it. Shared contract: extend it, never change what is here.
use super::annotate::SubtaskRow;
use super::client::CurationClient;
use super::document::{
    CURATION_ENCODING_VERSION, CurationGraph, CurationNode, DATASET_NODE_ID, Selection,
};
use super::population::{EpisodeSample, Population, PopulationReader};
use super::preview::{
    PreviewPass, PreviewPlan, PreviewProgress, PreviewReads, PreviewRows, ReadInput,
};
use super::session::{DocumentCommand, DocumentSession};
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::scoped_task::ScopedTask;
use crate::shell::state::StudioProjectState;
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use spiral_robotics_common::curation::{CurationDocument, CurationSummary};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::Place;
use std::collections::BTreeMap;

/// A save is refused while the document has another command in flight, so a
/// publication's save waits for it rather than dropping what it recorded.
const SAVE_WAIT_MS: u32 = 50;
const SAVE_WAIT_ROUNDS: usize = 200;

/// One quiet line per node that is measuring something right now.
pub(crate) type NodeActivity = BTreeMap<String, String>;

/// Where a node's long measurement stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunState {
    Idle,
    Running,
    /// Stop was pressed and the run is ending: nothing new starts, and what
    /// is in flight is dropped.
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
/// "Annotating 12 of 40 episodes" forever. Previews and publishing therefore
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

/// The graph travels inside the document as opaque text. Replacing that text
/// keeps the document's identity, name, and revision, so one encoded payload
/// is all that separates a draft from what Studio stored.
pub(super) trait CurationDraft {
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

/// What every node is interpreted over: the sample of episodes, read once per
/// dataset; what the preview has read over it this session; and whether the
/// curation just opened has yet to read back what its stored answers hold.
#[derive(Clone, Copy, PartialEq)]
struct Interpretation {
    sample: Resource<Result<Option<EpisodeSample>, String>>,
    reads: Signal<PreviewReads>,
    unrestored: Signal<bool>,
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
    session: DocumentSession,
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
    interpretation: Interpretation,
}

impl CurationController {
    pub(crate) fn use_controller() -> Self {
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
        let controller = use_hook(|| Self {
            session,
            payload,
            selected_node: Signal::new(Some(DATASET_NODE_ID.to_owned())),
            focus: Signal::new(None),
            runs: RunBook::new(),
            launch: Signal::new(None),
            interpretation: Interpretation {
                sample,
                reads: Signal::new(PreviewReads::default()),
                unrestored: Signal::new(false),
            },
        });
        controller.use_stored_publications();
        controller.use_restored_previews();
        controller
    }

    /// Once the sample of the curation just opened is read, restore what its
    /// previews read before: their answers are stored with the dataset, not
    /// in the document.
    fn use_restored_previews(self) {
        let mut unrestored = self.interpretation.unrestored;
        use_effect(move || {
            if !unrestored() || self.sample().is_none() {
                return;
            }
            unrestored.set(false);
            self.restore_below(DATASET_NODE_ID);
        });
    }

    /// Restore the previews of the measurements reading what `above` hands
    /// on, each as its own run reading what its stored answers hold, then
    /// those below each one: a preview reads from its parent's, so a chain
    /// restores from the top down while branches restore side by side.
    fn restore_below(self, above: &str) {
        let plans = self.with_reading(|graph, reader| {
            let Some(reader) = reader else {
                return Vec::new();
            };
            graph
                .measures_below(above)
                .into_iter()
                .filter_map(|node| {
                    let pending = PreviewProgress::of(reader, node.id()).pending();
                    let plan = PreviewPlan::of(reader, node.id(), &pending).ok()?;
                    Some((node.id().to_owned(), plan))
                })
                .collect::<Vec<_>>()
        });
        for (node, plan) in plans {
            let restored = node.clone();
            self.start_run(&node, move |run| async move {
                PreviewPass::new(self, run, plan).restore().await;
                self.restore_below(&restored);
            });
        }
    }

    /// Keep the open curation's publications with what Studio stores, which
    /// another surface records too — Data unpublishing a view. The stored
    /// copy is read again whenever the catalog changes.
    fn use_stored_publications(self) {
        let open = use_memo(move || {
            self.session
                .with_draft(|document| (document.revision() > 0).then(|| document.id().to_owned()))
        });
        let stored = QueryPolicy::on_demand(QueryDomain::Catalog).use_resource(move || {
            let open = open();
            async move {
                match open {
                    Some(id) => CurationClient::current().load(&id).await.map(Some),
                    None => Ok(None),
                }
            }
        });
        use_effect(move || {
            if let Some(Ok(Some(stored))) = &*stored.read() {
                self.take_publications(stored);
            }
        });
    }

    /// Take `stored`'s publications when it is the open curation stored
    /// since, differing from what this copy last stored by its publications
    /// alone: the graph's edits and what the preview read stay. Any other
    /// change is another writer's, which the next save refuses as before.
    fn take_publications(self, stored: &CurationDocument) {
        let Ok(stored_graph) = Self::graph_of(stored) else {
            return;
        };
        let publications_alone = self.session.peek_baseline(|baseline| {
            baseline.id() == stored.id()
                && stored.revision() > baseline.revision()
                && Self::graph_of(baseline).is_ok_and(|graph| graph.same_recipe(&stored_graph))
        });
        if publications_alone
            && self
                .edit(|payload| {
                    payload.take_publications(&stored_graph);
                    Ok(())
                })
                .is_ok()
        {
            self.session.adopt_stored(stored.clone());
        }
    }

    /// The episodes this curation is interpreted over: `None` while they are
    /// read, and for a graph without a dataset. A resource keeps its last
    /// value while it reads the next, so the sample of the dataset before —
    /// another curation's, just after switching — is held back rather than
    /// listed as this one's.
    pub(super) fn sample(&self) -> Option<EpisodeSample> {
        let sample = self
            .interpretation
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
        self.interpretation
            .sample
            .read()
            .as_ref()
            .and_then(|loaded| loaded.as_ref().err())
            .cloned()
    }
    pub(super) fn reload_sample(mut self) {
        self.interpretation.sample.restart();
    }
    /// What `node` hands on, and what reaches it, once the sample is read.
    pub(super) fn populations(&self, node: &str) -> Option<(Population, Population)> {
        self.with_reading(|_, reader| {
            let reader = reader?;
            Some((reader.input(node), reader.of(node)))
        })
    }
    /// What `node`'s rule judges, and what it keeps of that, once the sample
    /// is read.
    pub(super) fn judged(&self, node: &str) -> Option<(Population, Population)> {
        self.with_reading(|_, reader| {
            let reader = reader?;
            Some((reader.judged(node), reader.of(node)))
        })
    }
    /// Replace `node`'s rule, when it still keeps by that measure.
    pub(super) fn set_rule(self, node: &str, rule: Selection) -> Result<(), String> {
        self.edit(|payload| payload.set_rule(node, rule))
    }
    /// Read the graph together with what it is interpreted over, once the
    /// sample is read.
    pub(super) fn with_reading<R>(
        &self,
        read: impl FnOnce(&CurationGraph, Option<&PopulationReader<'_>>) -> R,
    ) -> R {
        let sample = self.sample();
        let reads = self.interpretation.reads.read();
        self.with_payload(|payload| {
            let reader = sample
                .as_ref()
                .map(|sample| PopulationReader::new(payload, sample, &reads));
            read(payload, reader.as_ref())
        })
    }
    /// Hold what `node`'s preview of one episode answered from what went
    /// into it, for the session.
    pub(super) fn record_preview(
        mut self,
        node: &CurationNode,
        read_from: (&EpisodeId, ReadInput),
        read: Result<PreviewRows, String>,
    ) {
        self.interpretation
            .reads
            .write()
            .record(node, read_from, read);
    }
    /// The revision Studio holds the open curation at; 0 before its first save.
    pub(super) fn saved_revision(&self) -> u64 {
        self.session.with_draft(CurationDocument::revision)
    }

    pub(crate) fn id(&self) -> String {
        self.session.with_draft(|document| document.id().to_owned())
    }
    pub(crate) fn name(&self) -> String {
        self.session
            .with_draft(|document| document.name().to_owned())
    }
    /// Name the draft as part of the edit that names it, such as choosing
    /// its dataset; it is saved with that edit.
    pub(crate) fn set_name(self, name: String) {
        self.session.set_name(name);
    }
    /// Rename the curation; a stored one is saved under the new name at once.
    pub(crate) fn rename(self, name: String, studio: StudioProjectState) {
        self.session.rename(name, studio);
    }
    /// Delete the saved curation `id`, open or not, and the views its stored
    /// copy publishes first, so no view outlives the recipe it was compiled
    /// from. The open one gives way to a blank graph.
    pub(crate) fn delete(self, id: String, studio: StudioProjectState) {
        self.session
            .delete(id, studio, move |opened| self.adopt(opened));
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
            self.forget_removed_reads();
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
    /// Let go of what the preview read for nodes the graph no longer holds.
    fn forget_removed_reads(mut self) {
        let payload = self.payload.peek();
        self.interpretation.reads.write().forget_removed(&payload);
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
    /// Whether a reload would lose work: edits not saved, or a run still
    /// reading. What the preview has read is not: its answers are stored, and
    /// read back when the curation opens again. Read without subscribing, for
    /// the moment the page is about to unload.
    pub(crate) fn holds_unkept_work(&self) -> bool {
        self.session.peek_unsaved_changes() || !self.runs.owned.peek().is_empty()
    }
    /// Whether this graph has ever been stored.
    pub(crate) fn is_stored(&self) -> bool {
        self.session.is_stored()
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
    /// Run `node`'s preview of `episodes` as the curation's own run, from
    /// the rows its parent's preview holds now; or say why it cannot run. A
    /// node already running is left alone.
    pub(super) fn run_preview(self, node: &str, episodes: &[EpisodeId]) -> Result<(), String> {
        let plan = self.with_reading(|_, reader| {
            let reader = reader.ok_or("The episodes are still being read.")?;
            PreviewPlan::of(reader, node, episodes).map_err(|error| error.message().to_owned())
        })?;
        self.start_run(node, move |run| async move {
            PreviewPass::new(self, run, plan).run().await;
        });
        Ok(())
    }
    /// Where `node`'s preview stands over the episodes that reach it, once
    /// the sample is read.
    pub(super) fn progress(&self, node: &str) -> Option<PreviewProgress> {
        self.with_reading(|_, reader| reader.map(|reader| PreviewProgress::of(reader, node)))
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
    pub(super) fn find_similar(mut self, annotation: &str, row: &SubtaskRow, from: Option<Place>) {
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
    pub(crate) fn documents(&self) -> Vec<CurationSummary> {
        self.session.summaries()
    }
    /// Curation opens on a blank graph, so the first read lists what is saved
    /// without opening any of it.
    pub(crate) fn refresh_documents(self) {
        self.session.refresh_summaries();
    }

    /// Start a new, unsaved, blank graph.
    pub(crate) fn create(self) {
        self.session
            .install(DocumentSession::fresh(), move |opened| self.adopt(opened));
    }
    pub(crate) fn open(self, id: String) {
        if self.session.command().is_some() {
            return;
        }
        self.session.open(id, move |opened| self.adopt(opened));
    }
    /// Take the document's graph as the one on screen, and restore its
    /// previews once its sample is read. A payload this Studio cannot read
    /// refuses the document rather than emptying the canvas.
    fn adopt(mut self, document: &CurationDocument) -> Result<(), String> {
        let graph = Self::graph_of(document)?;
        self.runs.cancel_all();
        self.interpretation.reads.set(PreviewReads::default());
        self.interpretation.unrestored.set(true);
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
    /// publish just recorded reaches Studio even while a save is running.
    pub(super) async fn save_when_idle(self, studio: StudioProjectState) {
        self.idle().await;
        self.save(studio);
    }
    /// Save what is drawn, if anything is unsaved, and say once Studio has
    /// answered whether it is now all saved. A refused save is reported like
    /// any other.
    pub(super) async fn saved(self, studio: StudioProjectState) -> bool {
        if self.unsaved_changes() {
            self.save_when_idle(studio).await;
            self.idle().await;
        }
        !self.unsaved_changes()
    }
    /// Wait, ten seconds at most, for the document command in flight.
    async fn idle(self) {
        for _ in 0..SAVE_WAIT_ROUNDS {
            if self.command().is_none() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(SAVE_WAIT_MS).await;
        }
    }
    /// Drop local edits and return to the last saved or loaded state. The
    /// selection only moves when the node it named is no longer there.
    pub(crate) fn discard(mut self) {
        self.session.discard(|baseline| {
            self.payload.set(Self::graph_of(baseline)?);
            self.runs.cancel_removed(&self.payload.peek());
            self.forget_removed_reads();
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
