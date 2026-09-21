//! Browser-owned workflow document, authoring state, and run observations.
use crate::activities::workflow::WorkflowClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::runs::RunLedger;
use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_workbench::PanelId;
use spiral_robotics_common::workflow::{
    WorkflowAgentArtifactReceipt, WorkflowRunSnapshot, WorkflowSpec, WorkflowSummary,
    WorkflowValidationReport,
};

use super::agent::{WorkflowAgentState, WorkflowAssistant};
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::workflow::document::{
    WorkflowDocument, WorkflowPlan, WorkflowRun, WorkflowSqlPlan,
};

pub(crate) const ASSISTANT_PANEL_ID: &str = "workflow-assistant";
pub(crate) const RESULTS_PANEL_ID: &str = "workflow-results";
pub(crate) const CANVAS_PANEL_ID: &str = "workflow-canvas";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowPendingCommand {
    Saving,
    Selecting,
    Starting,
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
    active_panel: Signal<Option<PanelId>>,
    document: WorkflowDefinitionSignals,
    lifecycle: WorkflowLifecycleSignals,
    assistant: WorkflowAssistant,
    agent_drafts: Signal<BTreeMap<AgentDraftKey, String>>,
}

#[derive(Clone, Copy, PartialEq)]
struct WorkflowDefinitionSignals {
    baseline: Signal<WorkflowDocument>,
    draft: Signal<WorkflowSpec>,
    runs: Signal<Vec<WorkflowRun>>,
    conflict: Signal<bool>,
    selected_run: Signal<Option<String>>,
    planned_sql: Signal<Option<WorkflowSqlPlan>>,
}
#[derive(Clone, Copy, PartialEq)]
struct WorkflowLifecycleSignals {
    loaded: Signal<bool>,
    command: Signal<Option<WorkflowPendingCommand>>,
    load_error: Signal<Option<String>>,
    workflows: Signal<Vec<WorkflowSummary>>,
}

impl WorkflowController {
    pub(super) fn use_controller() -> Self {
        let initial = use_hook(Self::fresh_document);
        let initial_spec = initial.workflow().clone();
        use_hook(|| Self {
            active_panel: Signal::new(None),
            assistant: WorkflowAssistant::new(initial_spec.id()),
            agent_drafts: Signal::new(BTreeMap::new()),
            document: WorkflowDefinitionSignals {
                baseline: Signal::new(initial),
                draft: Signal::new(initial_spec),
                runs: Signal::new(Vec::new()),
                conflict: Signal::new(false),
                selected_run: Signal::new(None),
                planned_sql: Signal::new(None),
            },
            lifecycle: WorkflowLifecycleSignals {
                loaded: Signal::new(false),
                command: Signal::new(None),
                load_error: Signal::new(None),
                workflows: Signal::new(Vec::new()),
            },
        })
    }

    pub(super) fn spec(&self) -> WorkflowSpec {
        self.document.draft.read().to_owned()
    }

    pub(super) fn snapshot(&self) -> WorkflowRunSnapshot {
        let runs = self.visible_run().into_iter().collect::<Vec<_>>();
        Self::project_run(&self.document.draft.read(), &runs)
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
        let selected = self.document.selected_run.read();
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
        let id = self.document.draft.read().id().to_owned();
        let mut runs = self
            .document
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
            self.document.selected_run.set(Some(id));
        }
    }
    pub(super) fn observe_run(mut self, run: WorkflowRun) {
        self.document.runs.accept_runs(vec![run]);
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
        self.document.selected_run.set(None);
        self.observe_run(run.clone());
        QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
        Ok(run)
    }
    pub(super) fn editor_status(&self) -> Option<WorkflowEditorStatus> {
        self.loaded()
            .then(|| WorkflowEditorStatus::new(self.spec(), self.snapshot()))
    }

    pub(super) fn validation(&self) -> WorkflowValidationReport {
        self.document.draft.read().validate()
    }

    pub(super) fn loaded(&self) -> bool {
        *self.lifecycle.loaded.read()
    }

    pub(super) fn load_error(&self) -> Option<String> {
        self.lifecycle.load_error.read().to_owned()
    }

    pub(super) fn command(&self) -> Option<WorkflowPendingCommand> {
        *self.lifecycle.command.read()
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
    pub(super) fn agent_error(&self) -> Option<String> {
        self.agent_state().session().error().map(str::to_owned)
    }
    pub(super) const fn agent_drafts(self) -> Signal<BTreeMap<AgentDraftKey, String>> {
        self.agent_drafts
    }
    pub(super) fn workflows(&self) -> Vec<WorkflowSummary> {
        let mut workflows = self.lifecycle.workflows.read().clone();
        for run in self.document.runs.read().iter() {
            if !workflows
                .iter()
                .any(|workflow| workflow.id() == run.workflow().id())
            {
                workflows.push(WorkflowSummary::new(
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

    pub(super) fn create_from_dataset(
        mut self,
        source: spiral_robotics_common::datasets::DatasetSummary,
    ) {
        if self.workflow_selection_reason().is_some() {
            return;
        }
        self.install(Self::fresh_document());
        let mut draft = self.document.draft.write();
        draft.set_name(source.selection().label());
        draft.set_sql(format!(
            "SELECT id AS episode_id, * FROM spql.mcap.episodes('{}')",
            source.selection().sql_name().replace('\'', "''")
        ));
    }

    pub(super) fn plan_for(&self, workflow: &WorkflowSpec) -> Option<WorkflowPlan> {
        self.document
            .planned_sql
            .peek()
            .as_ref()
            .and_then(|planned| planned.for_workflow(workflow))
            .cloned()
    }

    pub(super) fn remember_plan(mut self, workflow: &WorkflowSpec, plan: WorkflowPlan) {
        let current = self.spec();
        if current.id() == workflow.id() && current.execution_sql() == workflow.execution_sql() {
            self.document
                .planned_sql
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

    pub(super) fn set_name(mut self, name: String) {
        self.document.draft.write().set_name(name);
    }

    fn dirty(&self) -> bool {
        *self.document.draft.read() != *self.document.baseline.read().workflow()
    }

    pub(super) fn revision_conflict(&self) -> bool {
        *self.document.conflict.read()
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
            .document
            .runs
            .read()
            .iter()
            .any(|run| run.snapshot().phase().active())
        {
            return Some("A workflow is already running. Open its query to inspect it.".to_owned());
        }
        let draft = self.document.draft.read();
        if draft.sql().trim().is_empty() {
            return Some("Ask the Assistant to build this workflow before running.".to_owned());
        }
        drop(draft);
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
        if self.revision_conflict() {
            return Some(
                "The workflow changed in another session. Reload the current query before editing again."
                    .to_owned(),
            );
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

    pub(super) fn open_agent_receipt(mut self, receipt: WorkflowAgentArtifactReceipt) {
        let Some(panel) = self.receipt_panel(&receipt) else {
            return;
        };
        self.active_panel.set(Some(panel));
    }

    pub(super) fn reload_current(mut self) {
        self.lifecycle.load_error.set(None);
        let id = self.spec().id().to_owned();
        let known = self.workflows().iter().any(|workflow| workflow.id() == id);
        if !self.loaded() {
            self.load_initial();
        } else if known {
            self.open_document(Some(id));
        } else {
            self.install(Self::fresh_document());
        }
    }

    pub(super) fn unsaved_changes(&self) -> bool {
        self.dirty()
            || self.assistant.conversation() != *self.document.baseline.read().conversation()
    }
    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.document.draft.origin_scope(), task);
    }
    fn fresh_document() -> WorkflowDocument {
        WorkflowDocument::new(WorkflowSpec::new(
            format!(
                "workflow-{}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * u64::MAX as f64) as u64
            ),
            "Untitled workflow",
        ))
    }
    fn install(mut self, document: WorkflowDocument) {
        self.assistant.install(document.conversation().clone());
        self.document
            .planned_sql
            .set(document.planned_sql().cloned());
        self.document.draft.set(document.workflow().clone());
        self.document.baseline.set(document);
        self.document.conflict.set(false);
        self.active_panel.set(None);
        self.document.selected_run.set(None);
        self.lifecycle.loaded.set(true);
        self.lifecycle.load_error.set(None);
    }
    pub(super) fn replace_authored_query(
        mut self,
        expected: &WorkflowSpec,
        workflow: WorkflowSpec,
    ) -> Result<(), String> {
        self.document
            .draft
            .write()
            .replace_authored(expected, workflow)?;
        Ok(())
    }
    pub(super) fn save(mut self, studio: StudioProjectState) {
        if self.save_disabled_reason().is_some() {
            return;
        }
        let submitted =
            WorkflowDocument::new(self.spec()).with_conversation(self.assistant.conversation());
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Saving));
        self.spawn(async move {
            let client = WorkflowClient::current();
            let saved = async {
                let plan = if submitted.workflow().sql().trim().is_empty() {
                    None
                } else {
                    let plan = self.plan_query(submitted.workflow()).await?;
                    Some(WorkflowSqlPlan::new(submitted.workflow(), plan))
                };
                client.save(&submitted.with_plan(plan)).await
            }.await;
            match saved {
                Ok(saved) => {
                    // A save acknowledges its input. Edits made while it was in flight stay local.
                    if self.spec().id() == saved.workflow().id() {
                        self.document
                            .draft
                            .write()
                            .acknowledge_save(saved.workflow());
                        self.document.baseline.set(saved);
                        self.document.conflict.set(false);
                    }
                    if let Ok(workflows) = client.list().await {
                        self.lifecycle.workflows.set(workflows);
                    }
                }
                Err(error) => {
                    self.document.conflict.set(error.code() == spiral_robotics_common::command_failure::CommandFailureCode::Conflict);
                    studio.report_from(MessageSource::Workflow, error.to_string());
                }
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(super) fn discard(self, _studio: StudioProjectState) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.document.baseline.peek().clone();
        self.install(baseline);
    }
    pub(super) fn run(mut self, studio: StudioProjectState) {
        if let Some(reason) = self.run_disabled_reason() {
            studio.report_warning_from(MessageSource::Workflow, reason);
            return;
        }
        let workflow = self.spec();
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Starting));
        self.spawn(async move {
            match WorkflowClient::current().start(&workflow).await {
                Ok(run) => {
                    self.document.selected_run.set(None);
                    self.observe_run(run);
                }
                Err(error) => studio.report_from(MessageSource::Workflow, error.to_string()),
            }
            self.lifecycle.command.set(None);
            QueryRefresh::current().invalidate(&[QueryDomain::Workflow]);
        });
    }
    pub(super) fn select_workflow(self, workflow_id: Option<String>, _studio: StudioProjectState) {
        if self.workflow_selection_reason().is_none() {
            self.open_document(workflow_id);
        }
    }
    fn open_document(mut self, id: Option<String>) {
        let Some(id) = id else {
            self.install(Self::fresh_document());
            return;
        };
        if !self
            .lifecycle
            .workflows
            .peek()
            .iter()
            .any(|workflow| workflow.id() == id)
        {
            let run = self
                .document
                .runs
                .peek()
                .iter()
                .filter(|run| run.workflow().id() == id)
                .max_by_key(|run| run.snapshot().started_ms())
                .cloned();
            if let Some(run) = run {
                let mut document = WorkflowDocument::new(run.workflow().clone());
                document.workflow_mut().set_revision(0);
                self.install(document);
                return;
            }
        }
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Selecting));
        self.spawn(async move {
            match WorkflowClient::current().load(&id).await {
                Ok(document) => self.install(document),
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
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
    fn load_initial(mut self) {
        self.lifecycle
            .command
            .set(Some(WorkflowPendingCommand::Selecting));
        self.spawn(async move {
            let client = WorkflowClient::current();
            let result = async {
                let workflows = client.list().await?;
                self.lifecycle.workflows.set(workflows.clone());
                let document = match workflows.first() {
                    Some(workflow) => client.load(workflow.id()).await?,
                    None => Self::fresh_document(),
                };
                Ok::<_, crate::client::transport::CommandError>((workflows, document))
            }
            .await;
            match result {
                Ok((workflows, document)) => {
                    self.lifecycle.workflows.set(workflows);
                    self.install(document);
                }
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(super) fn use_polling(self, is_active: ReadSignal<bool>) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        use_effect(move || {
            if sessions.studio_available()
                && !self.loaded()
                && self.command().is_none()
                && self.load_error().is_none()
            {
                self.load_initial();
            }
        });
        use_hook(move || {
            self.spawn(async move {
                self.assistant.connect().await;
            });
        });
        let active = use_memo(move || {
            self.document
                .runs
                .read()
                .iter()
                .any(|run| run.snapshot().phase().active())
                || self.command() == Some(WorkflowPendingCommand::Starting)
        });
        let runs = QueryPolicy::new(QueryDomain::Workflow, 500)
            .with_activity(active.into())
            .use_when(
                move || is_active() && sessions.studio_available(),
                move || {
                    let active = self
                        .document
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
        self.document.runs.observe(runs);
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
