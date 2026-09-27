//! One owner for a Studio document's life: what is open, what the operator has
//! changed, what Studio has stored, and the commands that move between them —
//! open, save, rename, delete, discard.
//!
//! Curation opens one saved document at a time, edits it in the browser, and
//! saves it back. That lifecycle is this module; the activity's controller
//! keeps only what is its own — the graph's selection and focus — and holds a
//! [`DocumentSession`] beside it.
//!
//! A controller wires itself in by implementing [`StudioDocument`] for the value
//! it edits and calling [`DocumentSession::use_session`] once at mount. Every
//! command that replaces what is open takes an `on_install` hook, because the
//! document is never the whole of what an activity shows: Curation decodes the
//! graph payload there, and refuses the document by returning `Err` when that
//! payload cannot be read.

use dioxus::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub(crate) use crate::client::documents::DocumentSummary;

use crate::client::documents::DocumentClient;
use crate::components::scoped_task::ScopedTask;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

/// A value an activity opens, edits, and saves as one document.
///
/// The three constants say where Studio keeps these documents and what it calls
/// them. The methods are everything the session needs and no more: to identify
/// one, name one, rename one, start a new one, and take a save's answer without
/// discarding edits typed while it was in flight.
pub(crate) trait StudioDocument:
    Clone + PartialEq + Serialize + DeserializeOwned + 'static
{
    /// What Studio calls one of these, such as `curation`.
    /// A browser-minted id begins with it and the session's own messages use it.
    const NOUN: &'static str;
    /// The path segment these are served under, such as `curations`. See
    /// [`DocumentClient`] for the three requests it spells.
    const COLLECTION: &'static str;
    /// Which activity a failed save is reported as coming from.
    const REPORTS_AS: MessageSource;

    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn set_name(&mut self, name: String);

    /// A never-saved document under `id`, named however this activity names an
    /// untitled one. The session mints `id`; the implementation supplies
    /// everything else an empty document of this kind starts with.
    fn fresh(id: String) -> Self;

    /// Take what a save returned without losing edits made while it was in
    /// flight: keep the local content and adopt the stored revision. A save
    /// that answers about a different document is ignored.
    fn acknowledge_save(&mut self, saved: &Self);

    /// The stored revision this copy was loaded or saved at; 0 for one never
    /// stored.
    fn revision(&self) -> u64;

    /// Whether Studio has stored this document at least once.
    fn is_stored(&self) -> bool {
        self.revision() > 0
    }
}

/// Whether what is on screen matches what Studio has stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentState {
    Saved,
    Draft,
    /// Never stored and not yet changed: nothing is lost, and nothing is
    /// saved either.
    New,
}

impl DocumentState {
    pub(crate) const fn of(unsaved: bool, stored: bool) -> Self {
        match (unsaved, stored) {
            (true, _) => Self::Draft,
            (false, true) => Self::Saved,
            (false, false) => Self::New,
        }
    }
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Saved => "Saved",
            Self::Draft => "Unsaved changes",
            Self::New => "Not saved yet",
        }
    }
    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Saved => "is-saved",
            Self::Draft => "is-draft",
            Self::New => "is-new",
        }
    }
}

/// The one document operation Studio is performing. Running the document is a
/// separate ledger and is not one of these.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentCommand {
    /// Reading the saved list, or one saved document.
    Opening,
    Saving,
    Deleting,
}

impl DocumentCommand {
    /// What a document bar says while Studio is busy with it. `noun` is the
    /// activity's own word for its document, as [`StudioDocument::NOUN`] spells
    /// it.
    pub(crate) fn waiting_for(self, noun: &str) -> String {
        match self {
            Self::Opening => format!("Opening a saved {noun}…"),
            Self::Saving => format!("Saving this {noun}…"),
            Self::Deleting => format!("Deleting this {noun}…"),
        }
    }
}

/// A browser-minted document id: the kind, the moment, and enough randomness
/// that two documents made in the same millisecond are still distinct.
struct DocumentIdentity;

impl DocumentIdentity {
    fn mint(noun: &str) -> String {
        format!(
            "{noun}-{}-{:x}",
            js_sys::Date::now() as u64,
            (js_sys::Math::random() * u64::MAX as f64) as u64
        )
    }
}

/// What is open, what has changed, and the commands that move between them.
///
/// Held by an activity's controller as one field and passed down as a prop. It
/// is `Copy`: every method takes `self` by value and writes through signals, so
/// a handler can capture it without cloning.
pub(crate) struct DocumentSession<Doc: StudioDocument> {
    /// What the operator is editing.
    draft: Signal<Doc>,
    /// What Studio last stored or served.
    baseline: Signal<Doc>,
    summaries: Signal<Vec<DocumentSummary>>,
    command: Signal<Option<DocumentCommand>>,
    /// The last list or load failure, cleared by the next success.
    load_error: Signal<Option<String>>,
}

impl<Doc: StudioDocument> Clone for DocumentSession<Doc> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Doc: StudioDocument> Copy for DocumentSession<Doc> {}
impl<Doc: StudioDocument> PartialEq for DocumentSession<Doc> {
    fn eq(&self, other: &Self) -> bool {
        self.draft == other.draft
            && self.baseline == other.baseline
            && self.summaries == other.summaries
            && self.command == other.command
            && self.load_error == other.load_error
    }
}

impl<Doc: StudioDocument> DocumentSession<Doc> {
    /// One session per mounted activity. It starts on a fresh, never-saved
    /// document, so a workspace has something to draw before Studio answers.
    pub(crate) fn use_session() -> Self {
        let initial = use_hook(Self::fresh);
        use_hook(|| Self {
            draft: Signal::new(initial.clone()),
            baseline: Signal::new(initial),
            summaries: Signal::new(Vec::new()),
            command: Signal::new(None),
            load_error: Signal::new(None),
        })
    }

    /// A new, never-saved document of this kind.
    pub(crate) fn fresh() -> Doc {
        Doc::fresh(DocumentIdentity::mint(Doc::NOUN))
    }

    pub(crate) fn draft(&self) -> Doc {
        self.draft.read().clone()
    }

    /// Read the draft without cloning it.
    pub(crate) fn with_draft<R>(&self, read: impl FnOnce(&Doc) -> R) -> R {
        read(&self.draft.read())
    }

    /// Every edit goes through here, so the draft stays one value and one
    /// place decides what "changed" means.
    pub(crate) fn edit<R>(self, change: impl FnOnce(&mut Doc) -> R) -> R {
        let mut draft = self.draft;
        change(&mut draft.write())
    }

    pub(crate) fn set_name(self, name: String) {
        self.edit(|document| document.set_name(name));
    }

    pub(crate) fn summaries(&self) -> Vec<DocumentSummary> {
        self.summaries.read().clone()
    }

    pub(crate) fn load_error(&self) -> Option<String> {
        self.load_error.read().clone()
    }

    pub(crate) fn command(&self) -> Option<DocumentCommand> {
        *self.command.read()
    }

    pub(crate) fn unsaved_changes(&self) -> bool {
        *self.draft.read() != *self.baseline.read()
    }
    /// `unsaved_changes` without subscribing to either document.
    pub(crate) fn peek_unsaved_changes(&self) -> bool {
        *self.draft.peek() != *self.baseline.peek()
    }
    /// Whether what Studio last stored or served was ever stored at all.
    pub(crate) fn is_stored(&self) -> bool {
        self.baseline.read().is_stored()
    }

    /// Run `task` on the scope that owns this session, so a command outlives the
    /// view that started it but is cancelled with the workspace.
    pub(crate) fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        ScopedTask::spawn(self.draft.origin_scope(), task);
    }

    /// Make `document` the open one. `on_install` runs first and may refuse it:
    /// returning `Err` leaves the draft alone and reports the reason as a load
    /// failure, which is how Curation rejects a payload it cannot decode. On
    /// `Ok` the hook has already reinstalled whatever the activity keeps beside
    /// the document — a conversation, a selection, a decoded graph.
    pub(crate) fn install(
        mut self,
        document: Doc,
        on_install: impl FnOnce(&Doc) -> Result<(), String>,
    ) {
        if let Err(problem) = on_install(&document) {
            self.load_error.set(Some(problem));
            return;
        }
        self.draft.set(document.clone());
        self.baseline.set(document);
        self.load_error.set(None);
    }

    /// Open the saved document `id`, or a fresh one when `id` is `None`.
    pub(crate) fn open(
        mut self,
        id: Option<String>,
        on_install: impl FnOnce(&Doc) -> Result<(), String> + 'static,
    ) {
        self.load_error.set(None);
        let Some(id) = id else {
            self.install(Self::fresh(), on_install);
            return;
        };
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            match DocumentClient::<Doc>::current(Doc::COLLECTION)
                .load(&id)
                .await
            {
                Ok(document) => self.install(document, on_install),
                Err(error) => self.load_error.set(Some(error.to_string())),
            }
            self.command.set(None);
        });
    }

    /// List what is saved without opening any of it: the first read of a
    /// workspace that starts on a blank document rather than on the most
    /// recent one, as Curation does.
    pub(crate) fn refresh_summaries(mut self) {
        if self.command().is_some() {
            return;
        }
        self.load_error.set(None);
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            match DocumentClient::<Doc>::current(Doc::COLLECTION).list().await {
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
    pub(crate) fn save(mut self, studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let submitted = self.draft();
        self.command.set(Some(DocumentCommand::Saving));
        self.spawn(async move {
            let client = DocumentClient::<Doc>::current(Doc::COLLECTION);
            match client.save(&submitted).await {
                Ok(saved) => {
                    if self.draft.peek().id() == saved.id() {
                        self.draft.write().acknowledge_save(&saved);
                        self.baseline.set(saved);
                    }
                    if let Ok(summaries) = client.list().await {
                        self.summaries.set(summaries);
                    }
                }
                Err(error) => studio.report_from(Doc::REPORTS_AS, error.to_string()),
            }
            self.command.set(None);
        });
    }

    /// Rename the document where Studio stores it too, as a rename is
    /// everywhere else: a stored document is saved under the new name at
    /// once, from what Studio stored, so edits not yet saved stay unsaved
    /// and only the name moves. A document never stored takes the name for
    /// its first save. A refused rename puts the old name back and is
    /// reported like any other command failure.
    pub(crate) fn rename(mut self, name: String, studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let previous = self.draft.peek().name().to_owned();
        self.set_name(name.clone());
        let mut stored = self.baseline.peek().clone();
        if !stored.is_stored() {
            return;
        }
        stored.set_name(name);
        self.command.set(Some(DocumentCommand::Saving));
        self.spawn(async move {
            let client = DocumentClient::<Doc>::current(Doc::COLLECTION);
            match client.save(&stored).await {
                Ok(saved) => {
                    if self.draft.peek().id() == saved.id() {
                        self.draft.write().acknowledge_save(&saved);
                        self.baseline.set(saved);
                    }
                    if let Ok(summaries) = client.list().await {
                        self.summaries.set(summaries);
                    }
                }
                Err(error) => {
                    if self.draft.peek().id() == stored.id() {
                        self.set_name(previous);
                    }
                    studio.report_from(Doc::REPORTS_AS, error.to_string());
                }
            }
            self.command.set(None);
        });
    }

    /// Delete the saved document `id`, open or not. Its stored copy is read
    /// first and handed to `prepare` — Curation drops the views it publishes
    /// — whose failure stops the delete and is reported like any command
    /// failure. Then it is deleted at the revision just read, so a save
    /// landing in between is refused rather than lost; if it is the open
    /// document, a fresh one opens in its place.
    pub(crate) fn delete<Prepared>(
        mut self,
        id: String,
        studio: StudioProjectState,
        prepare: impl FnOnce(&Doc) -> Prepared + 'static,
        on_install: impl FnOnce(&Doc) -> Result<(), String> + 'static,
    ) where
        Prepared: std::future::Future<Output = Result<(), String>> + 'static,
    {
        if self.command().is_some() {
            return;
        }
        self.command.set(Some(DocumentCommand::Deleting));
        self.spawn(async move {
            let client = DocumentClient::<Doc>::current(Doc::COLLECTION);
            let deleted = match client.load(&id).await {
                Ok(stored) => match prepare(&stored).await {
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
                Err(reason) => studio.report_from(Doc::REPORTS_AS, reason),
            }
            self.command.set(None);
        });
    }

    /// Drop local edits and return to what Studio last stored or served.
    pub(crate) fn discard(self, on_install: impl FnOnce(&Doc) -> Result<(), String>) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.baseline.peek().clone();
        self.install(baseline, on_install);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The summary a switcher reads is the two things every document endpoint
    /// sends; the revision a curation also carries is not one of them.
    #[test]
    fn a_summary_reads_a_curation_listing() {
        let body = serde_json::to_string(&spiral_robotics_common::curation::CurationSummary::new(
            "c1",
            "Throughput",
            7,
        ))
        .unwrap();
        let summary = serde_json::from_str::<DocumentSummary>(&body).unwrap();
        assert_eq!(summary, DocumentSummary::new("c1", "Throughput"));
    }
}
