//! One owner for the open curation's life: what is open, what the operator
//! has changed, what Studio has stored, and the commands that move between
//! them — open, save, rename, delete, discard.
//!
//! Curation opens one saved document at a time, edits it in the browser, and
//! saves it back. The controller keeps only what is its own — the decoded
//! graph, its selection and focus — and holds a [`DocumentSession`] beside
//! it. Every command that replaces what is open takes an `on_install` hook,
//! because the document is never the whole of what Curation shows: the
//! controller decodes the graph payload there, and refuses the document by
//! returning `Err` when that payload cannot be read.

use dioxus::prelude::*;
use spiral_robotics_common::curation::{CurationDocument, CurationSummary};

use super::client::CurationClient;
use super::document::{CURATION_ENCODING_VERSION, CurationGraph};
use super::graph::PublishedViews;
use super::state::CurationDraft as _;
use crate::components::scoped_task::ScopedTask;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

pub(super) const UNTITLED: &str = "Untitled curation";

/// Whether what is on screen matches what Studio has stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DocumentState {
    Saved,
    Draft,
    /// Never stored and not yet changed: nothing is lost, and nothing is
    /// saved either.
    New,
}

impl DocumentState {
    pub(super) const fn of(unsaved: bool, stored: bool) -> Self {
        match (unsaved, stored) {
            (true, _) => Self::Draft,
            (false, true) => Self::Saved,
            (false, false) => Self::New,
        }
    }
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Saved => "Saved",
            Self::Draft => "Unsaved changes",
            Self::New => "Not saved yet",
        }
    }
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Saved => "is-saved",
            Self::Draft => "is-draft",
            Self::New => "is-new",
        }
    }
}

/// The one document operation Studio is performing. Running the curation is
/// a separate ledger and is not one of these.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentCommand {
    /// Reading the saved list, or one saved curation.
    Opening,
    Saving,
    Deleting,
}

impl DocumentCommand {
    /// What the document bar says while Studio is busy with the curation.
    pub(super) fn waiting_for(self) -> String {
        match self {
            Self::Opening => "Opening a saved curation…",
            Self::Saving => "Saving this curation…",
            Self::Deleting => "Deleting this curation…",
        }
        .to_owned()
    }
}

/// A browser-minted curation id: the moment, and enough randomness that two
/// curations made in the same millisecond are still distinct.
struct DocumentIdentity;

impl DocumentIdentity {
    fn mint() -> String {
        format!(
            "curation-{}-{:x}",
            js_sys::Date::now() as u64,
            (js_sys::Math::random() * u64::MAX as f64) as u64
        )
    }
}

/// What is open, what has changed, and the commands that move between them.
///
/// Held by the controller as one field. It is `Copy`: every method takes
/// `self` by value and writes through signals, so a handler can capture it
/// without cloning.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct DocumentSession {
    /// What the operator is editing.
    draft: Signal<CurationDocument>,
    /// What Studio last stored or served.
    baseline: Signal<CurationDocument>,
    summaries: Signal<Vec<CurationSummary>>,
    command: Signal<Option<DocumentCommand>>,
    /// The last list or load failure, cleared by the next success.
    load_error: Signal<Option<String>>,
}

impl DocumentSession {
    /// One session per mounted Curation. It starts on a fresh, never-saved
    /// curation, so a workspace has something to draw before Studio answers.
    pub(super) fn use_session() -> Self {
        let initial = use_hook(Self::fresh);
        use_hook(|| Self {
            draft: Signal::new(initial.clone()),
            baseline: Signal::new(initial),
            summaries: Signal::new(Vec::new()),
            command: Signal::new(None),
            load_error: Signal::new(None),
        })
    }

    /// A new, never-saved curation with an empty graph.
    pub(super) fn fresh() -> CurationDocument {
        CurationDocument::new(
            DocumentIdentity::mint(),
            UNTITLED,
            CURATION_ENCODING_VERSION,
            "{}",
        )
        .carrying(&CurationGraph::new(None))
    }

    /// Read the draft without cloning it.
    pub(super) fn with_draft<R>(&self, read: impl FnOnce(&CurationDocument) -> R) -> R {
        read(&self.draft.read())
    }

    /// Every edit goes through here, so the draft stays one value and one
    /// place decides what "changed" means.
    pub(super) fn edit<R>(self, change: impl FnOnce(&mut CurationDocument) -> R) -> R {
        let mut draft = self.draft;
        change(&mut draft.write())
    }

    pub(super) fn set_name(self, name: String) {
        self.edit(|document| document.set_name(name));
    }

    pub(super) fn summaries(&self) -> Vec<CurationSummary> {
        self.summaries.read().clone()
    }

    pub(super) fn load_error(&self) -> Option<String> {
        self.load_error.read().clone()
    }

    pub(super) fn command(&self) -> Option<DocumentCommand> {
        *self.command.read()
    }

    pub(super) fn unsaved_changes(&self) -> bool {
        *self.draft.read() != *self.baseline.read()
    }
    /// `unsaved_changes` without subscribing to either document.
    pub(super) fn peek_unsaved_changes(&self) -> bool {
        *self.draft.peek() != *self.baseline.peek()
    }
    /// Whether what Studio last stored or served was ever stored at all.
    pub(super) fn is_stored(&self) -> bool {
        self.baseline.read().revision() > 0
    }
    /// Read what Studio last stored or served, without subscribing to it.
    pub(super) fn peek_baseline<R>(&self, read: impl FnOnce(&CurationDocument) -> R) -> R {
        read(&self.baseline.peek())
    }

    /// Take `stored`, the open curation as Studio now stores it, as what
    /// Studio last stored: the draft keeps its content and takes the stored
    /// revision, so edits made while a save was in flight survive. Another
    /// curation is ignored.
    pub(super) fn adopt_stored(mut self, stored: CurationDocument) {
        if self.draft.peek().id() == stored.id() {
            self.draft.write().set_revision(stored.revision());
            self.baseline.set(stored);
        }
    }

    /// Run `task` on the scope that owns this session, so a command outlives
    /// the view that started it but is cancelled with the workspace.
    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        ScopedTask::spawn(self.draft.origin_scope(), task);
    }

    /// Make `document` the open one. `on_install` runs first and may refuse
    /// it: returning `Err` leaves the draft alone and reports the reason as a
    /// load failure. On `Ok` the hook has already reinstalled the decoded
    /// graph.
    pub(super) fn install(
        mut self,
        document: CurationDocument,
        on_install: impl FnOnce(&CurationDocument) -> Result<(), String>,
    ) {
        if let Err(problem) = on_install(&document) {
            self.load_error.set(Some(problem));
            return;
        }
        self.draft.set(document.clone());
        self.baseline.set(document);
        self.load_error.set(None);
    }

    /// Open the saved curation `id`.
    pub(super) fn open(
        mut self,
        id: String,
        on_install: impl FnOnce(&CurationDocument) -> Result<(), String> + 'static,
    ) {
        self.load_error.set(None);
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            match CurationClient::current().load(&id).await {
                Ok(document) => self.install(document, on_install),
                Err(error) => self.load_error.set(Some(error.to_string())),
            }
            self.command.set(None);
        });
    }

    /// List what is saved without opening any of it: Curation starts on a
    /// blank graph rather than on the most recent one.
    pub(super) fn refresh_summaries(mut self) {
        if self.command().is_some() {
            return;
        }
        self.load_error.set(None);
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            match CurationClient::current().list().await {
                Ok(summaries) => self.summaries.set(summaries),
                Err(error) => self.load_error.set(Some(error.to_string())),
            }
            self.command.set(None);
        });
    }

    /// Store the draft. The answer is acknowledged into the draft rather than
    /// replacing it, so edits made while the save was in flight survive; a
    /// refused save — including one another writer beat — is reported through
    /// Studio's notifications like any other command failure.
    pub(super) fn save(mut self, studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let submitted = self.draft.peek().clone();
        self.command.set(Some(DocumentCommand::Saving));
        self.spawn(async move {
            let client = CurationClient::current();
            match client.save(&submitted).await {
                Ok(saved) => {
                    self.adopt_stored(saved);
                    if let Ok(summaries) = client.list().await {
                        self.summaries.set(summaries);
                    }
                }
                Err(error) => studio.report_from(MessageSource::Curation, error.to_string()),
            }
            self.command.set(None);
        });
    }

    /// Rename the curation where Studio stores it too, as a rename is
    /// everywhere else: a stored curation is saved under the new name at
    /// once, from what Studio stored, so edits not yet saved stay unsaved
    /// and only the name moves. A curation never stored takes the name for
    /// its first save. A refused rename puts the old name back and is
    /// reported like any other command failure.
    pub(super) fn rename(mut self, name: String, studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let previous = self.draft.peek().name().to_owned();
        self.set_name(name.clone());
        let mut stored = self.baseline.peek().clone();
        if stored.revision() == 0 {
            return;
        }
        stored.set_name(name);
        self.command.set(Some(DocumentCommand::Saving));
        self.spawn(async move {
            let client = CurationClient::current();
            match client.save(&stored).await {
                Ok(saved) => {
                    self.adopt_stored(saved);
                    if let Ok(summaries) = client.list().await {
                        self.summaries.set(summaries);
                    }
                }
                Err(error) => {
                    if self.draft.peek().id() == stored.id() {
                        self.set_name(previous);
                    }
                    studio.report_from(MessageSource::Curation, error.to_string());
                }
            }
            self.command.set(None);
        });
    }

    /// Delete the saved curation `id`, open or not, and the views its stored
    /// copy publishes first, so no view outlives the recipe it was compiled
    /// from; a view the catalog refuses to drop stops the delete and is
    /// reported like any command failure. Then it is deleted at the revision
    /// just read, so a save landing in between is refused rather than lost;
    /// if it is the open curation, a fresh one opens in its place.
    pub(super) fn delete(
        mut self,
        id: String,
        studio: StudioProjectState,
        on_install: impl FnOnce(&CurationDocument) -> Result<(), String> + 'static,
    ) {
        if self.command().is_some() {
            return;
        }
        self.command.set(Some(DocumentCommand::Deleting));
        self.spawn(async move {
            let client = CurationClient::current();
            let deleted = match client.load(&id).await {
                Ok(stored) => match PublishedViews::of(stored.payload()).drop_all().await {
                    Ok(()) => client
                        .delete(stored.id(), stored.revision())
                        .await
                        .map_err(|error| error.to_string()),
                    Err(reason) => Err(reason),
                },
                Err(error) => Err(error.to_string()),
            };
            match deleted {
                Ok(()) => {
                    if self.draft.peek().id() == id {
                        self.install(Self::fresh(), on_install);
                    }
                    if let Ok(summaries) = client.list().await {
                        self.summaries.set(summaries);
                    }
                }
                Err(reason) => studio.report_from(MessageSource::Curation, reason),
            }
            self.command.set(None);
        });
    }

    /// Drop local edits and return to what Studio last stored or served.
    pub(super) fn discard(self, on_install: impl FnOnce(&CurationDocument) -> Result<(), String>) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.baseline.peek().clone();
        self.install(baseline, on_install);
    }
}
