//! Browser-owned workflow document, authoring state, and run observations.
use crate::activities::workflow::WorkflowClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::runs::RunLedger;
use crate::components::document_session::{
    DocumentCommand, DocumentSession, DocumentSummary, StudioDocument,
};
use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_workbench::PanelId;
use spiral_robotics_common::workflow::{
    WorkflowAgentArtifactReceipt, WorkflowRunSnapshot, WorkflowSpec, WorkflowValidationReport,
};

use super::agent::{WorkflowAgentState, WorkflowAssistant};
use super::explore::graph::WorkflowView;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::workflow::document::{
    WorkflowDocument, WorkflowPlan, WorkflowRun, WorkflowSqlPlan,
};

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";

impl StudioDocument for WorkflowDocument {
    const NOUN: &'static str = "workflow";
    const COLLECTION: &'static str = "workflows";
    const REPORTS_AS: MessageSource = MessageSource::Workflow;

    fn id(&self) -> &str {
        self.workflow().id()
    }
    fn set_name(&mut self, name: String) {
        self.workflow_mut().set_name(name);
    }
    fn fresh(id: String) -> Self {
        Self::new(WorkflowSpec::new(id, "Untitled workflow"))
    }
    fn acknowledge_save(&mut self, saved: &Self) {
        self.workflow_mut().acknowledge_save(saved.workflow());
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentPendingCommand {
    Sending,
    Cancelling,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AgentDraftKey(String);

impl AgentDraftKey {
    pub(super) fn new(workflow_id: &str) -> Self {
        Self(workflow_id.to_owned())
    }

    pub(super) fn clear_if_unchanged(
        &self,
        drafts: &mut BTreeMap<AgentDraftKey, String>,
        submitted: &str,
    ) -> bool {
        if drafts.get(self).map(String::as_str) != Some(submitted) {
            return false;
        }
        drafts.remove(self);
        true
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WorkflowController {
    session: DocumentSession<WorkflowDocument>,
    active_panel: Signal<Option<PanelId>>,
    runs: WorkflowRunSignals,
    assistant: WorkflowAssistant,
    agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>,
    /// The plan Spiral last returned for some SQL. A derivation of the draft,
    /// not an edit to it: it travels with a save but never makes one necessary.
    planned_sql: Signal<Option<WorkflowSqlPlan>>,
}

#[derive(Clone, Copy, PartialEq)]
struct WorkflowRunSignals {
    runs: Signal<Vec<WorkflowRun>>,
    selected_run: Signal<Option<String>>,
    /// A run Studio has accepted but not yet reported back.
    starting: Signal<bool>,
}

impl WorkflowController {
    pub(super) fn use_controller() -> Self {
        let session = DocumentSession::<WorkflowDocument>::use_session();
        let controller = use_hook(|| Self {
            session,
            active_panel: Signal::new(None),
            assistant: WorkflowAssistant::new(
                &session.with_draft(|document| document.id().to_owned()),
            ),
            agent_drafts: Signal::new(BTreeMap::new()),
            runs: WorkflowRunSignals {
                runs: Signal::new(Vec::new()),
                selected_run: Signal::new(None),
                starting: Signal::new(false),
            },
            planned_sql: Signal::new(None),
        });
        // The conversation is part of the document, and the assistant owns it
        // while a turn is running. Carrying it back into the draft is what
        // makes "Unsaved changes" one comparison and a save one value.
        use_effect(move || {
            let conversation = controller.assistant.conversation();
            if session.with_draft(|document| document.conversation() != &conversation) {
                session.edit(|document| {
                    let carried = document.clone().with_conversation(conversation);
                    *document = carried;
                });
            }
        });
        controller
    }

    pub(super) fn spec(&self) -> WorkflowSpec {
        self.session
            .with_draft(|document| document.workflow().clone())
    }

    pub(super) fn snapshot(&self) -> WorkflowRunSnapshot {
        let runs = self.visible_run().into_iter().collect::<Vec<_>>();
        Self::project_run(&self.spec(), &runs)
    }
    fn project_run(draft: &WorkflowSpec, runs: &[WorkflowRun]) -> WorkflowRunSnapshot {
        let Some(run) = runs
            .iter()
            .filter(|run| run.workflow().id() == draft.id())
            .max_by_key(|run| run.snapshot().started_ms())
        else {
            return WorkflowRunSnapshot::idle_for(draft);
        };
        let mut snapshot = run.snapshot().clone();
        snapshot.set_result_stale(!draft.accepts_output_from(run.workflow()));
        snapshot
    }
    fn visible_run(&self) -> Option<WorkflowRun> {
        let runs = self.workflow_runs();
        let selected = self.runs.selected_run.read();
        selected
            .as_ref()
            .and_then(|id| {
                runs.iter()
                    .find(|run| run.snapshot().run_id() == Some(id.as_str()))
            })
            .or_else(|| runs.last())
            .cloned()
    }
    pub(super) fn workflow_runs(&self) -> Vec<WorkflowRun> {
        let id = self.session.with_draft(|document| document.id().to_owned());
        let mut runs = self
            .runs
            .runs
            .read()
            .iter()
            .filter(|run| run.workflow().id() == id)
            .cloned()
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.snapshot().started_ms());
        runs
    }
    pub(super) fn select_run(mut self, id: String) {
        if self
            .workflow_runs()
            .iter()
            .any(|run| run.snapshot().run_id() == Some(id.as_str()))
        {
            self.runs.selected_run.set(Some(id));
        }
    }
    pub(super) fn observe_run(mut self, run: WorkflowRun) {
        self.runs.runs.accept_runs(vec![run]);
    }
    pub(super) async fn submit_query(
        mut self,
        expected: &WorkflowSpec,
    ) -> Result<WorkflowRun, String> {
        if self.spec() != *expected || self.command().is_some() {
            return Err("The draft changed or a document operation is in progress.".into());
        }
        let run = WorkflowClient::current().start(expected).await.map_err(|error| {
            format!("{error}. Do not blindly retry a submission: inspect this workflow's runs to check whether it was admitted.")
        })?;
        self.runs.selected_run.set(None);
        self.observe_run(run.clone());
        QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
        Ok(run)
    }
    pub(super) fn editor_status(&self) -> Option<WorkflowEditorStatus> {
        self.loaded()
            .then(|| WorkflowEditorStatus::new(self.spec(), self.snapshot()))
    }

    pub(super) fn validation(&self) -> WorkflowValidationReport {
        self.session
            .with_draft(|document| document.workflow().validate())
    }

    pub(super) fn loaded(&self) -> bool {
        self.session.loaded()
    }

    pub(super) fn load_error(&self) -> Option<String> {
        self.session.load_error()
    }

    pub(super) fn command(&self) -> Option<DocumentCommand> {
        self.session.command()
    }

    /// A run is under way, or Studio has accepted a start it has not yet
    /// reported back.
    pub(super) fn running(&self) -> bool {
        self.snapshot().phase().active() || *self.runs.starting.read()
    }

    pub(super) fn agent_state(&self) -> WorkflowAgentState {
        self.assistant.snapshot(self.loaded())
    }
    pub(super) fn agent_loaded(&self) -> bool {
        self.assistant.loaded()
    }
    pub(super) fn agent_command(&self) -> Option<AgentPendingCommand> {
        self.agent_state()
            .session()
            .phase()
            .active()
            .then_some(AgentPendingCommand::Sending)
    }
    pub(super) const fn agent_drafts(self) -> Signal<BTreeMap<AgentDraftKey, String>> {
        self.agent_drafts
    }
    /// The saved workflows, plus any workflow a run observed that Studio has
    /// never stored: a run's query is openable even though it was never saved.
    pub(super) fn workflows(&self) -> Vec<DocumentSummary> {
        let mut workflows = self.session.summaries();
        for run in self.runs.runs.read().iter() {
            if !workflows
                .iter()
                .any(|workflow| workflow.id() == run.workflow().id())
            {
                workflows.push(DocumentSummary::new(
                    run.workflow().id(),
                    format!("{} (run)", run.workflow().name()),
                ));
            }
        }
        workflows
    }

    pub(super) fn workflow_selection_reason(&self) -> Option<String> {
        if !self.loaded() || !self.agent_loaded() {
            return Some("Loading workflows…".to_owned());
        }
        if self.command().is_some() || self.agent_command().is_some() {
            return Some("Wait for the current action to finish.".to_owned());
        }
        if self.agent_state().session().phase().active() {
            return Some("Stop the active work before switching workflows.".to_owned());
        }
        if self.unsaved_changes() {
            return Some("Save or discard your changes before switching workflows.".to_owned());
        }
        if !crate::shell::project::ProjectSessionSignals::current().studio_available() {
            return Some("Reconnect to Studio before switching workflows.".to_owned());
        }
        None
    }

    /// A new workflow that already reads one dataset: the dataset names it and
    /// its episodes are the query's source.
    pub(super) fn create_from_dataset(
        self,
        source: spiral_robotics_common::datasets::DatasetSummary,
    ) {
        if self.workflow_selection_reason().is_some() {
            return;
        }
        self.session
            .install(DocumentSession::fresh(), move |opened| self.adopt(opened));
        self.session.edit(|document| {
            let draft = document.workflow_mut();
            draft.set_name(source.selection().label());
            draft.set_sql(format!(
                "SELECT id AS episode_id, * FROM spql.mcap.episodes('{}')",
                source.selection().sql_name().replace('\'', "''")
            ));
        });
    }

    pub(super) fn plan_for(&self, workflow: &WorkflowSpec) -> Option<WorkflowPlan> {
        self.planned_sql
            .peek()
            .as_ref()
            .and_then(|planned| planned.for_workflow(workflow))
            .cloned()
    }

    pub(super) fn remember_plan(mut self, workflow: &WorkflowSpec, plan: WorkflowPlan) {
        let current = self.spec();
        if current.id() == workflow.id() && current.execution_sql() == workflow.execution_sql() {
            self.planned_sql
                .set(Some(WorkflowSqlPlan::new(workflow, plan)));
        }
    }

    pub(super) async fn plan_query(
        self,
        workflow: &WorkflowSpec,
    ) -> Result<WorkflowPlan, crate::client::CommandError> {
        if let Some(plan) = self.plan_for(workflow) {
            return Ok(plan);
        }
        let plan = WorkflowClient::current().plan(workflow).await?;
        self.remember_plan(workflow, plan.clone());
        Ok(plan)
    }

    pub(super) fn active_panel(&self) -> Option<PanelId> {
        self.active_panel.read().clone()
    }

    pub(super) fn run_disabled_reason(&self) -> Option<String> {
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved Workflow: {error}"));
        }
        if !self.loaded() {
            return Some("Loading the saved Workflow…".to_owned());
        }
        if self.command().is_some() {
            return Some("A Workflow command is in progress.".to_owned());
        }
        if self.agent_state().session().phase().active() || self.agent_command().is_some() {
            return Some("The Workflow Assistant is using this query.".to_owned());
        }
        if self
            .runs
            .runs
            .read()
            .iter()
            .any(|run| run.snapshot().phase().active())
        {
            return Some("A workflow is already running. Open its query to inspect it.".to_owned());
        }
        if self.spec().sql().trim().is_empty() {
            return Some("Ask the Assistant to build this workflow before running.".to_owned());
        }
        self.validation()
            .errors()
            .first()
            .map(|issue| issue.message().to_owned())
    }

    pub(super) fn save_disabled_reason(&self) -> Option<String> {
        if !self.loaded() || !self.agent_loaded() {
            return Some("Loading workflow…".into());
        }
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved Workflow: {error}"));
        }
        if self.command().is_some() {
            return Some("Wait for the current document operation.".to_owned());
        }
        if self.agent_state().session().phase().active() || self.agent_command().is_some() {
            return Some("Wait for the active Assistant turn before saving.".to_owned());
        }
        None
    }

    pub(super) fn activate_panel(mut self, id: PanelId) {
        self.active_panel.set(Some(id));
    }

    pub(super) fn receipt_panel(&self, receipt: &WorkflowAgentArtifactReceipt) -> Option<PanelId> {
        (receipt.revision() == self.spec().revision()).then(|| PanelId::from(CANVAS_PANEL_ID))
    }

    /// A workflow is never the whole of what the Agent view shows: installing
    /// one reinstalls its conversation, its stored plan, and clears the panel
    /// and run the workflow that was open had selected.
    fn adopt(mut self, document: &WorkflowDocument) -> Result<(), String> {
        self.assistant.install(document.conversation().clone());
        self.planned_sql.set(document.planned_sql().cloned());
        self.active_panel.set(None);
        self.runs.selected_run.set(None);
        Ok(())
    }

    pub(super) fn unsaved_changes(&self) -> bool {
        self.session.unsaved_changes()
    }

    pub(super) fn set_name(self, name: String) {
        self.session.set_name(name);
    }

    pub(super) fn replace_authored_query(
        self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String> {
        self.session
            .edit(|document| document.workflow_mut().replace_authored(expected, workflow))
    }

    /// Store the draft, carrying the plan Spiral returned for exactly this SQL
    /// so reopening the workflow shows it without asking again.
    pub(super) fn save(self, studio: StudioProjectState) {
        if self.save_disabled_reason().is_some() {
            return;
        }
        let planned = self
            .plan_for(&self.spec())
            .map(|plan| WorkflowSqlPlan::new(&self.spec(), plan));
        self.session.edit(|document| {
            let carried = document.clone().with_plan(planned);
            *document = carried;
        });
        self.session.save(studio);
    }
    pub(super) fn discard(self, _studio: StudioProjectState) {
        self.session.discard(move |baseline| self.adopt(baseline));
    }
    pub(super) fn run(mut self, studio: StudioProjectState) {
        if let Some(reason) = self.run_disabled_reason() {
            studio.report_warning_from(MessageSource::Workflow, reason);
            return;
        }
        let workflow = self.spec();
        self.runs.starting.set(true);
        self.session.spawn(async move {
            match WorkflowClient::current().start(&workflow).await {
                Ok(run) => {
                    self.runs.selected_run.set(None);
                    self.observe_run(run);
                }
                Err(error) => studio.report_from(MessageSource::Workflow, error.to_string()),
            }
            self.runs.starting.set(false);
            QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
        });
    }
    pub(super) fn select_workflow(self, workflow_id: Option<String>, _studio: StudioProjectState) {
        if self.workflow_selection_reason().is_some() {
            return;
        }
        let Some(id) = workflow_id else {
            self.session.open(None, move |opened| self.adopt(opened));
            return;
        };
        // A run carries the exact query it submitted, so a workflow Studio
        // never stored is still openable — at revision 0, as a new document.
        let observed = self
            .session
            .summaries()
            .iter()
            .all(|workflow| workflow.id() != id)
            .then(|| {
                self.runs
                    .runs
                    .peek()
                    .iter()
                    .filter(|run| run.workflow().id() == id)
                    .max_by_key(|run| run.snapshot().started_ms())
                    .map(|run| WorkflowDocument::new(run.workflow().clone().with_revision(0)))
            })
            .flatten();
        match observed {
            Some(document) => self
                .session
                .install(document, move |opened| self.adopt(opened)),
            None => self
                .session
                .open(Some(id), move |opened| self.adopt(opened)),
        }
    }
    pub(super) fn send_agent(mut self, text: String, _studio: StudioProjectState) {
        if !self.agent_state().configured()
            || self.command().is_some()
            || self.agent_command().is_some()
        {
            return;
        }
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.assistant.send(self, text.clone());
        AgentDraftKey::new(self.spec().id())
            .clear_if_unchanged(&mut self.agent_drafts.write(), &text);
    }
    pub(super) fn cancel_agent(self, _studio: StudioProjectState) {
        self.assistant.cancel();
    }
    pub(super) fn retry_agent(self, studio: StudioProjectState) {
        if let Some(message) =
            self.agent_state()
                .session()
                .messages()
                .iter()
                .rev()
                .find(|message| {
                    message.role() == spiral_robotics_common::workflow::WorkflowAgentRole::User
                })
        {
            self.send_agent(message.text().to_owned(), studio);
        }
    }
    pub(super) fn use_polling(self, is_active: ReadSignal<bool>, view: Signal<WorkflowView>) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        use_effect(move || {
            if sessions.studio_available()
                && !self.loaded()
                && self.command().is_none()
                && self.load_error().is_none()
            {
                self.session.load_initial(move |opened| self.adopt(opened));
            }
        });
        use_hook(move || {
            self.session.spawn(async move {
                self.assistant.connect().await;
            });
        });
        // The open view decides whether the run list is worth re-reading, and
        // how often; `None` is the whole answer for a view that runs nothing.
        let cadence = use_memo(move || {
            let run_active = self
                .runs
                .runs
                .read()
                .iter()
                .any(|run| run.snapshot().phase().active());
            view().run_poll_interval_ms(run_active)
        });
        let runs = QueryPolicy::measured(QueryDomain::Workflow, cadence.into()).use_when(
            move || cadence().is_some() && is_active() && sessions.studio_available(),
            move || {
                let active = self
                    .runs
                    .runs
                    .peek()
                    .iter()
                    .find(|run| run.snapshot().phase().active())
                    .and_then(|run| run.snapshot().run_id())
                    .map(str::to_owned);
                async move {
                    let client = WorkflowClient::current();
                    match active {
                        Some(id) => client.run(&id).await.map(|run| vec![run]),
                        None => client.runs().await,
                    }
                }
            },
        );
        self.use_run_observations(runs);
    }

    fn use_run_observations(
        self,
        runs: Resource<Result<Vec<WorkflowRun>, crate::client::CommandError>>,
    ) {
        self.runs.runs.observe(runs);
    }
}

trait WorkflowDraft {
    fn replace_authored(
        &mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String>;
    fn acknowledge_save(&mut self, saved: &WorkflowSpec);
}

impl WorkflowDraft for WorkflowSpec {
    fn replace_authored(
        &mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String> {
        if self != expected || workflow.id() != expected.id() {
            return Err("The draft changed during authoring. Your edits were preserved.".into());
        }
        *self = workflow;
        Ok(())
    }
    fn acknowledge_save(&mut self, saved: &WorkflowSpec) {
        if self.id() == saved.id() {
            self.set_revision(saved.revision());
        }
    }
}

/// What the Workflow activity's status bar reports, which is whichever of its
/// two views is open: the Agent's SQL document and run phase, or the
/// exploration and what it has measured.
#[derive(Clone, PartialEq)]
pub(crate) enum WorkflowViewStatus {
    /// Boxed: a whole SQL document beside three counted fields would make
    /// every copy of this status the size of the larger one.
    Agent(Box<WorkflowEditorStatus>),
    Exploration(super::explore::state::ExplorationMeasure),
}

#[derive(Clone, PartialEq)]
pub(crate) struct WorkflowEditorStatus {
    workflow: WorkflowSpec,
    snapshot: WorkflowRunSnapshot,
}
impl WorkflowEditorStatus {
    pub(crate) fn new(workflow: WorkflowSpec, snapshot: WorkflowRunSnapshot) -> Self {
        Self { workflow, snapshot }
    }
    pub(crate) fn workflow(&self) -> &WorkflowSpec {
        &self.workflow
    }
    pub(crate) fn snapshot(&self) -> &WorkflowRunSnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod run_observation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::workflow::WorkflowRunPhase;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_inflight_save_updates_the_baseline_version_without_losing_new_edits() {
        let mut draft = WorkflowSpec::default();
        let mut saved = draft.clone();
        saved.set_revision(1);
        draft.set_name("Typed during Save");
        draft.acknowledge_save(&saved);
        assert_eq!(draft.name(), "Typed during Save");
        assert_eq!(draft.revision(), 1);
        assert_ne!(draft, saved);
        let other = WorkflowSpec::new("other", "Another document").with_revision(20);
        draft.acknowledge_save(&other);
        assert_eq!(draft.revision(), 1);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn renaming_or_saving_keeps_results_current_but_sql_changes_do_not() {
        let workflow = WorkflowSpec::new("result", "Original").with_sql("SELECT 1");
        let mut snapshot = WorkflowRunSnapshot::started(&workflow, "run");
        snapshot.set_phase(WorkflowRunPhase::Completed);
        let runs = [WorkflowRun::new(workflow.clone(), snapshot)];
        let mut draft = workflow;
        draft.set_name("Renamed");
        draft.set_revision(2);
        assert!(!WorkflowController::project_run(&draft, &runs).result_stale());
        draft.set_sql("SELECT 2");
        assert!(WorkflowController::project_run(&draft, &runs).result_stale());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_late_assistant_proposal_preserves_manual_edits() {
        let expected = WorkflowSpec::default();
        let mut draft = expected.clone();
        let mut proposed = expected.clone();
        proposed.set_name("Assistant proposal");
        draft.set_name("User edit while planning");
        assert!(draft.replace_authored(&expected, proposed.clone()).is_err());
        assert_eq!(draft.name(), "User edit while planning");
        let latest = draft.clone();
        draft.replace_authored(&latest, proposed.clone()).unwrap();
        assert_eq!(draft, proposed);
    }
}
