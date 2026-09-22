//! Browser-owned recipe draft, saved baseline, submitted runs, and the
//! sheet's selection state.
use super::CurationClient;
use super::pipeline::PipelineSelection;
use crate::client::runs::{PENDING_RUN_ID, RunHistory, SubmittedRun};
use crate::components::document_session::{
    DocumentCommand, DocumentSession, DocumentSummary, StudioDocument,
};
use crate::components::run_strip::RunPhasePresentation;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use dioxus_workbench::PanelId;
use spiral_robotics_common::curation::facts::CurationFact;
use spiral_robotics_common::curation::interpretation::RuleProposal;
use spiral_robotics_common::curation::run::{CurationOutcome, CurationRun, CurationRunSnapshot};
use spiral_robotics_common::curation::{
    CurationSpec, CurationValidationReport, DEFAULT_CURATION_NAME,
};
use spiral_robotics_common::runs::RunPhase;

pub(crate) const RECIPE_PANEL_ID: &str = "curation-recipe";
pub(crate) const SHEET_PANEL_ID: &str = "curation-sheet";

impl StudioDocument for CurationSpec {
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
        Self::new(id, DEFAULT_CURATION_NAME)
    }
    fn acknowledge_save(&mut self, saved: &Self) {
        if self.id() == saved.id() {
            self.set_revision(saved.revision());
        }
    }
}

/// Which verdicts the sheet lays out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SheetFilter {
    #[default]
    All,
    Kept,
    Excluded,
}

impl SheetFilter {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Kept => "Kept",
            Self::Excluded => "Excluded",
        }
    }
    pub(super) const fn admits(self, kept: bool) -> bool {
        match self {
            Self::All => true,
            Self::Kept => kept,
            Self::Excluded => !kept,
        }
    }
}

/// How the sheet orders its tiles: as the dataset recorded them, or by the
/// measurement the tiles print, so the episodes near a threshold sit together.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SheetOrder {
    #[default]
    Recorded,
    Ascending,
    Descending,
}

impl SheetOrder {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Recorded => "Dataset order",
            Self::Ascending => "Lowest first",
            Self::Descending => "Highest first",
        }
    }
    const fn next(self) -> Self {
        match self {
            Self::Recorded => Self::Ascending,
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Recorded,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CurationController {
    session: DocumentSession<CurationSpec>,
    runs: RunHistory<CurationRun>,
    view: CurationViewSignals,
}

impl SubmittedRun for CurationRun {
    fn document_id(&self) -> &str {
        self.curation().id()
    }
    fn run_id(&self) -> Option<&str> {
        self.snapshot().run_id()
    }
    fn phase(&self) -> RunPhase {
        self.snapshot().phase()
    }
    fn started_ms(&self) -> u64 {
        self.snapshot().started_ms()
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CurationViewSignals {
    active_panel: Signal<Option<PanelId>>,
    selected_episode: Signal<Option<String>>,
    hovered_rule: Signal<Option<String>>,
    filter: Signal<SheetFilter>,
    /// The measurement every tile prints; `None` means the first declared one.
    lens: Signal<Option<String>>,
    order: Signal<SheetOrder>,
    pipeline_selection: Signal<PipelineSelection>,
}

impl CurationController {
    pub(super) fn use_controller() -> Self {
        let session = DocumentSession::use_session();
        use_hook(|| Self {
            session,
            runs: RunHistory::new(),
            view: CurationViewSignals {
                active_panel: Signal::new(None),
                selected_episode: Signal::new(None),
                hovered_rule: Signal::new(None),
                filter: Signal::new(SheetFilter::All),
                lens: Signal::new(None),
                order: Signal::new(SheetOrder::Recorded),
                pipeline_selection: Signal::new(PipelineSelection::Source),
            },
        })
    }

    pub(super) fn spec(&self) -> CurationSpec {
        self.session.draft()
    }
    /// Every recipe edit goes through here so the draft stays one value.
    pub(super) fn edit(self, change: impl FnOnce(&mut CurationSpec)) {
        self.session.edit(change);
    }
    pub(super) fn add_proposal(self, proposal: &RuleProposal) -> Result<(), String> {
        let identity = format!(
            "rule-{}-{:x}",
            js_sys::Date::now() as u64,
            (js_sys::Math::random() * u32::MAX as f64) as u64
        );
        let mut next = self.spec();
        proposal.apply(&mut next, &identity)?;
        self.edit(|spec| *spec = next);
        self.select_pipeline(PipelineSelection::Step(identity));
        Ok(())
    }
    fn validation(&self) -> CurationValidationReport {
        self.session.with_draft(CurationSpec::validate)
    }

    /// The run the sheet shows: the selected one, else the latest for this
    /// recipe, with staleness judged against the current draft.
    pub(super) fn snapshot(&self) -> CurationRunSnapshot {
        let visible = self.visible_run();
        self.session.with_draft(|draft| match visible {
            Some(run) => {
                let mut snapshot = run.snapshot().clone();
                snapshot.set_result_stale(!draft.accepts_output_from(run.curation()));
                snapshot
            }
            None => CurationRunSnapshot::default(),
        })
    }
    pub(super) fn outcome(&self) -> Option<CurationOutcome> {
        self.visible_run()
            .and_then(|run| run.snapshot().outcome().cloned())
    }
    /// The recipe that produced the visible verdicts.
    pub(super) fn judged_spec(&self) -> Option<CurationSpec> {
        self.visible_run().map(|run| run.curation().clone())
    }
    fn visible_run(&self) -> Option<CurationRun> {
        self.runs.visible(&self.document_id())
    }
    fn document_id(&self) -> String {
        self.session.with_draft(|draft| draft.id().to_owned())
    }
    pub(super) fn curation_runs(&self) -> Vec<CurationRun> {
        self.runs.for_document(&self.document_id())
    }
    pub(super) fn select_run(mut self, id: String) {
        if self.runs.select(&self.document_id(), id) {
            self.view.selected_episode.set(None);
        }
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
    pub(super) fn unsaved_changes(&self) -> bool {
        self.session.unsaved_changes()
    }
    pub(super) fn curations(&self) -> Vec<DocumentSummary> {
        self.session.summaries()
    }
    pub(super) fn set_name(self, name: String) {
        self.session.set_name(name);
    }
    pub(super) fn judging(&self) -> bool {
        self.snapshot().phase().active()
    }

    pub(super) fn active_panel(&self) -> Option<PanelId> {
        self.view.active_panel.read().clone()
    }
    pub(super) fn activate_panel(mut self, id: PanelId) {
        self.view.active_panel.set(Some(id));
    }
    pub(super) fn selected_episode(&self) -> Option<String> {
        self.view.selected_episode.read().clone()
    }
    /// The episode whose evidence the side sheet shows; `None` closes it.
    pub(super) fn select_episode(mut self, id: Option<String>) {
        if *self.view.selected_episode.peek() != id {
            self.view.selected_episode.set(id);
        }
    }
    /// The measurement the tiles print: the chosen one while the judged
    /// recipe still declares it, else the first declared measurement.
    pub(super) fn lens(&self, judged: &CurationSpec) -> Option<CurationFact> {
        let chosen = self.view.lens.read();
        chosen
            .as_deref()
            .and_then(|key| judged.facts().iter().find(|fact| fact.key() == key))
            .or_else(|| judged.facts().first())
            .cloned()
    }
    pub(super) fn set_lens(mut self, key: String) {
        self.view.lens.set(Some(key));
    }
    pub(super) fn order(&self) -> SheetOrder {
        *self.view.order.read()
    }
    pub(super) fn cycle_order(mut self) {
        let next = self.view.order.peek().next();
        self.view.order.set(next);
    }
    pub(super) fn hovered_rule(&self) -> Option<String> {
        self.view.hovered_rule.read().clone()
    }
    pub(super) fn set_hovered_rule(mut self, id: Option<String>) {
        if *self.view.hovered_rule.peek() != id {
            self.view.hovered_rule.set(id);
        }
    }
    pub(super) fn pipeline_selection(&self) -> PipelineSelection {
        self.view.pipeline_selection.read().clone()
    }
    /// Opening a stage lays out its own episodes: the filter and the open
    /// episode belong to the stage that was left, so both reset.
    pub(super) fn select_pipeline(mut self, selection: PipelineSelection) {
        if *self.view.pipeline_selection.peek() != selection {
            self.view.pipeline_selection.set(selection);
            self.view.filter.set(SheetFilter::All);
            self.view.selected_episode.set(None);
        }
    }
    pub(super) fn filter(&self) -> SheetFilter {
        *self.view.filter.read()
    }
    pub(super) fn set_filter(mut self, filter: SheetFilter) {
        self.view.filter.set(filter);
    }

    pub(super) fn run_disabled_reason(&self) -> Option<String> {
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved curation: {error}"));
        }
        if !self.loaded() {
            return Some("Loading the saved curation…".to_owned());
        }
        if self.command().is_some() {
            return Some("A curation command is in progress.".to_owned());
        }
        if self.runs.any_active() {
            return Some("A curation is already running.".to_owned());
        }
        None
    }
    /// What the recipe still lacks before it can run. Run stays enabled so
    /// the answer is one click away instead of hidden in a tooltip.
    pub(super) fn run_problem(&self) -> Option<String> {
        self.validation()
            .errors()
            .first()
            .map(|issue| format!("{} {}", issue.message(), issue.recovery()))
    }
    pub(super) fn save_disabled_reason(&self) -> Option<String> {
        if !self.loaded() {
            return Some("Loading curation…".into());
        }
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved curation: {error}"));
        }
        if self.command().is_some() {
            return Some("Wait for the current document operation.".to_owned());
        }
        if self.spec().name().trim().is_empty() {
            return Some("Name the curation before saving.".to_owned());
        }
        if !self.unsaved_changes() && self.spec().revision() > 0 {
            return Some("Everything is saved.".to_owned());
        }
        None
    }
    pub(super) fn selection_reason(&self) -> Option<String> {
        if !self.loaded() {
            return Some("Loading curations…".to_owned());
        }
        if self.command().is_some() {
            return Some("Wait for the current action to finish.".to_owned());
        }
        if self.unsaved_changes() {
            return Some("Save or discard your changes before switching curations.".to_owned());
        }
        if !crate::shell::project::ProjectSessionSignals::current().studio_available() {
            return Some("Reconnect to Studio before switching curations.".to_owned());
        }
        None
    }

    /// Everything the sheet shows besides the recipe belongs to the recipe
    /// that was open: a newly installed one starts with no run selected, no
    /// episode open, no hover, and the pipeline at its source.
    fn adopt(self, _curation: &CurationSpec) -> Result<(), String> {
        let mut view = self.view;
        self.runs.clear_selection();
        view.selected_episode.set(None);
        view.hovered_rule.set(None);
        view.filter.set(SheetFilter::All);
        view.lens.set(None);
        view.order.set(SheetOrder::Recorded);
        view.pipeline_selection.set(PipelineSelection::Source);
        Ok(())
    }

    pub(super) fn save(self, studio: StudioProjectState) {
        if self.save_disabled_reason().is_some() {
            return;
        }
        self.session.save(studio);
    }
    pub(super) fn discard(self) {
        self.session.discard(move |curation| self.adopt(curation));
    }
    pub(super) fn run(mut self, studio: StudioProjectState) {
        if self.run_disabled_reason().is_some() {
            return;
        }
        if let Some(problem) = self.run_problem() {
            studio.report_warning_from(MessageSource::Curation, problem);
            return;
        }
        let curation = self.spec();
        self.view.selected_episode.set(None);
        let placeholder = CurationRun::new(
            curation.clone(),
            CurationRunSnapshot::started(PENDING_RUN_ID)
                .with_started_ms(js_sys::Date::now() as u64),
        );
        let answer = self.runs.submit(placeholder, async move {
            CurationClient::current()
                .start(&curation)
                .await
                .map_err(|error| error.to_string())
        });
        self.session.spawn(async move {
            if let Err(error) = answer.await {
                studio.report_from(MessageSource::Curation, error);
            }
        });
    }
    pub(super) fn select_curation(self, id: Option<String>) {
        if self.selection_reason().is_some() {
            return;
        }
        self.session.open(id, move |curation| self.adopt(curation));
    }
    /// Read the open curation again after a failed load: the saved one when
    /// Studio still lists it, the first saved one when nothing is open yet,
    /// and a blank recipe when what was open is gone.
    pub(super) fn reload_current(self) {
        if !self.loaded() {
            self.session
                .load_initial(move |curation| self.adopt(curation));
            return;
        }
        let id = self.spec().id().to_owned();
        let known = self
            .session
            .summaries()
            .iter()
            .any(|summary| summary.id() == id);
        self.session
            .open(known.then_some(id), move |curation| self.adopt(curation));
    }

    pub(super) fn use_loading(self) {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        use_effect(move || {
            if sessions.studio_available()
                && !self.loaded()
                && self.command().is_none()
                && self.load_error().is_none()
            {
                self.session
                    .load_initial(move |curation| self.adopt(curation));
            }
        });
    }

    pub(super) fn editor_status(&self) -> Option<CurationEditorStatus> {
        self.loaded()
            .then(|| CurationEditorStatus::new(self.spec(), self.snapshot()))
    }
}

/// What the status bar says about the open curation.
#[derive(Clone, PartialEq)]
pub(crate) struct CurationEditorStatus {
    curation: CurationSpec,
    snapshot: CurationRunSnapshot,
}

impl CurationEditorStatus {
    fn new(curation: CurationSpec, snapshot: CurationRunSnapshot) -> Self {
        Self { curation, snapshot }
    }
    pub(crate) fn dataset_label(&self) -> String {
        self.curation
            .source()
            .map_or_else(|| "No dataset".to_owned(), |source| source.label())
    }
    /// The run's phase while it is news; a completed run speaks through
    /// the verdict count instead.
    pub(crate) fn phase_label(&self) -> String {
        match self.snapshot.phase() {
            RunPhase::Idle | RunPhase::Completed => String::new(),
            phase => phase.run_label().to_owned(),
        }
    }
    pub(crate) fn verdict_label(&self) -> String {
        match self.snapshot.outcome() {
            Some(outcome) => format!(
                "{} of {} kept",
                outcome.totals().kept(),
                outcome.totals().episodes()
            ),
            None => self.curation.name().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::curation::rules::{
        ConditionOp, ConditionValue, CurationRule, RuleCondition,
    };
    use spiral_robotics_common::datasets::DatasetSelection;

    fn recipe() -> CurationSpec {
        CurationSpec::new("cur", "Bright")
            .with_source(DatasetSelection::new(vec!["dataset_x".into()]).unwrap())
            .with_rule(CurationRule::new(
                "short",
                "",
                RuleCondition::new(
                    "duration_s",
                    ConditionOp::Below,
                    ConditionValue::Number(5.0),
                ),
            ))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_status_bar_reads_dataset_rules_and_the_verdict_count() {
        let spec = recipe();
        let idle = CurationEditorStatus::new(spec.clone(), CurationRunSnapshot::default());
        assert_eq!(idle.dataset_label(), "dataset_x");
        assert_eq!(idle.phase_label(), "");
        assert_eq!(idle.verdict_label(), "Bright");
        let mut snapshot = CurationRunSnapshot::started("run");
        snapshot.set_phase(RunPhase::Completed);
        snapshot.set_outcome(Some(CurationOutcome::new(
            spiral_robotics_common::curation::run::CurationTotals::new(40, 31, (40, false)),
            Vec::new(),
        )));
        let complete = CurationEditorStatus::new(spec.clone(), snapshot);
        assert_eq!(complete.phase_label(), "");
        assert_eq!(complete.verdict_label(), "31 of 40 kept");
        let mut running = CurationRunSnapshot::started("run");
        running.set_phase(RunPhase::Running);
        assert_eq!(
            CurationEditorStatus::new(spec, running).phase_label(),
            "Running"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_sheet_order_cycles_through_recorded_and_both_directions() {
        assert_eq!(SheetOrder::Recorded.next(), SheetOrder::Ascending);
        assert_eq!(SheetOrder::Ascending.next(), SheetOrder::Descending);
        assert_eq!(SheetOrder::Descending.next(), SheetOrder::Recorded);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_sheet_filter_admits_by_verdict() {
        assert!(SheetFilter::All.admits(true) && SheetFilter::All.admits(false));
        assert!(SheetFilter::Kept.admits(true) && !SheetFilter::Kept.admits(false));
        assert!(!SheetFilter::Excluded.admits(true) && SheetFilter::Excluded.admits(false));
    }
}
