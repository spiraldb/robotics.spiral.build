//! One owner for a Studio document's life: what is open, what the operator has
//! changed, what Studio has stored, and the commands that move between them —
//! open, save, discard.
//!
//! Workflow, Curation, and Explore each open one saved document at a time, edit
//! it in the browser, and save it back. That lifecycle is this module; an
//! activity's controller keeps only what is its own — the agent conversation,
//! the sheet's selection, the graph's focus — and holds a
//! [`DocumentSession`] beside it.
//!
//! A controller wires itself in by implementing [`StudioDocument`] for the value
//! it edits and calling [`DocumentSession::use_session`] once at mount. Every
//! command that replaces what is open takes an `on_install` hook, because the
//! document is never the whole of what an activity shows: Workflow reinstalls
//! its assistant conversation there, Curation resets the sheet's filter, lens,
//! order, and selection, and Explore decodes the graph payload — and refuses the
//! document by returning `Err` when that payload cannot be read.

use dioxus::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub(crate) use crate::client::documents::DocumentSummary;

use crate::client::documents::DocumentClient;
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
    /// What Studio calls one of these: `workflow`, `curation`, `exploration`.
    /// A browser-minted id begins with it and the session's own messages use it.
    const NOUN: &'static str;
    /// The path segment these are served under, such as `workflows`. See
    /// [`DocumentClient`] for the three requests it spells.
    const COLLECTION: &'static str;
    /// Which activity a failed save is reported as coming from.
    const REPORTS_AS: MessageSource;

    fn id(&self) -> &str;
    fn set_name(&mut self, name: String);

    /// A never-saved document under `id`, named however this activity names an
    /// untitled one. The session mints `id`; the implementation supplies
    /// everything else an empty document of this kind starts with.
    fn fresh(id: String) -> Self;

    /// Take what a save returned without losing edits made while it was in
    /// flight: keep the local content and adopt the stored revision. A save
    /// that answers about a different document is ignored.
    fn acknowledge_save(&mut self, saved: &Self);
}

/// Whether what is on screen matches what Studio has stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentState {
    Saved,
    Draft,
}

impl DocumentState {
    pub(crate) const fn of(unsaved: bool) -> Self {
        if unsaved { Self::Draft } else { Self::Saved }
    }
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Saved => "Saved",
            Self::Draft => "Unsaved changes",
        }
    }
    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Saved => "is-saved",
            Self::Draft => "is-draft",
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
}

impl DocumentCommand {
    /// What a document bar says while Studio is busy with it. `noun` is the
    /// activity's own word for its document, as [`StudioDocument::NOUN`] spells
    /// it.
    pub(crate) fn waiting_for(self, noun: &str) -> String {
        match self {
            Self::Opening => format!("Opening a saved {noun}…"),
            Self::Saving => format!("Saving this {noun}…"),
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
    loaded: Signal<bool>,
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
            && self.loaded == other.loaded
    }
}

impl<Doc: StudioDocument> DocumentSession<Doc> {
    /// One session per mounted activity. It starts on a fresh, never-saved
    /// document and stays unloaded until [`Self::load_initial`] runs, so a
    /// workspace has something to draw before Studio answers.
    pub(crate) fn use_session() -> Self {
        let initial = use_hook(Self::fresh);
        use_hook(|| Self {
            draft: Signal::new(initial.clone()),
            baseline: Signal::new(initial),
            summaries: Signal::new(Vec::new()),
            command: Signal::new(None),
            load_error: Signal::new(None),
            loaded: Signal::new(false),
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

    pub(crate) fn loaded(&self) -> bool {
        *self.loaded.read()
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

    /// Run `task` on the scope that owns this session, so a command outlives the
    /// view that started it but is cancelled with the workspace.
    pub(crate) fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.draft.origin_scope(), task);
    }

    /// Make `document` the open one. `on_install` runs first and may refuse it:
    /// returning `Err` leaves the draft alone and reports the reason as a load
    /// failure, which is how Explore rejects a payload it cannot decode. On
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
        self.loaded.set(true);
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

    /// The first read of a mounted workspace: list what is saved and open the
    /// first of them, or a fresh document when nothing is saved yet.
    pub(crate) fn load_initial(
        mut self,
        on_install: impl FnOnce(&Doc) -> Result<(), String> + 'static,
    ) {
        self.load_error.set(None);
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            let client = DocumentClient::<Doc>::current(Doc::COLLECTION);
            let opened = async {
                let summaries = client.list().await?;
                let document = match summaries.first() {
                    Some(summary) => client.load(summary.id()).await?,
                    None => Self::fresh(),
                };
                Ok::<_, crate::client::CommandError>((summaries, document))
            }
            .await;
            match opened {
                Ok((summaries, document)) => {
                    self.summaries.set(summaries);
                    self.install(document, on_install);
                }
                Err(error) => self.load_error.set(Some(error.to_string())),
            }
            self.command.set(None);
        });
    }

    /// List what is saved without opening any of it: the first read of a
    /// workspace that starts on a blank document rather than on the most
    /// recent one, as Explore does.
    pub(crate) fn refresh_summaries(mut self) {
        if self.command().is_some() {
            return;
        }
        self.load_error.set(None);
        self.command.set(Some(DocumentCommand::Opening));
        self.spawn(async move {
            match DocumentClient::<Doc>::current(Doc::COLLECTION).list().await {
                Ok(summaries) => {
                    self.summaries.set(summaries);
                    self.loaded.set(true);
                }
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

    /// One summary type reads every collection's list. The two things a
    /// switcher shows are the two every document endpoint sends, and the extra
    /// field an exploration carries is simply not one of them.
    #[test]
    fn one_summary_reads_every_collection() {
        let listed = [
            serde_json::to_string(&spiral_robotics_common::workflow::WorkflowSummary::new(
                "w1",
                "Throughput",
            ))
            .unwrap(),
            serde_json::to_string(&spiral_robotics_common::curation::CurationSummary::new(
                "w1",
                "Throughput",
            ))
            .unwrap(),
            serde_json::to_string(
                &spiral_robotics_common::exploration::ExplorationSummary::new(
                    "w1",
                    "Throughput",
                    7,
                ),
            )
            .unwrap(),
        ];
        for body in listed {
            let summary = serde_json::from_str::<DocumentSummary>(&body).unwrap();
            assert_eq!(summary, DocumentSummary::new("w1", "Throughput"), "{body}");
        }
    }
}
