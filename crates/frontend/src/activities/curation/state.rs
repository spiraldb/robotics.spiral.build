//! Browser-owned recipe draft, saved baseline, run observations, and the
//! sheet's selection state.
use super::CurationClient;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::client::runs::RunLedger;
use crate::components::run_phase::RunPhasePresentation;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use dioxus_workbench::PanelId;
use spiral_robotics_common::curation::facts::CurationFact;
use spiral_robotics_common::curation::run::{CurationOutcome, CurationRun, CurationRunSnapshot};
use spiral_robotics_common::curation::{
    CurationSpec, CurationSummary, CurationValidationReport, DEFAULT_CURATION_NAME,
};
use spiral_robotics_common::runs::RunPhase;

pub(crate) const RECIPE_PANEL_ID: &str = "curation-recipe";
pub(crate) const SHEET_PANEL_ID: &str = "curation-sheet";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CurationPendingCommand {
    Saving,
    Selecting,
    Starting,
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
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Kept => "Kept",
            Self::Excluded => "Excluded",
        }
    }
    pub(crate) const fn admits(self, kept: bool) -> bool {
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
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Recorded => "Dataset order",
            Self::Ascending => "Lowest first",
            Self::Descending => "Highest first",
        }
    }
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Recorded => Self::Ascending,
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Recorded,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CurationController {
    document: CurationDocumentSignals,
    runs: CurationRunSignals,
    lifecycle: CurationLifecycleSignals,
    view: CurationViewSignals,
}

#[derive(Clone, Copy, PartialEq)]
struct CurationDocumentSignals {
    baseline: Signal<CurationSpec>,
    draft: Signal<CurationSpec>,
    conflict: Signal<bool>,
}

#[derive(Clone, Copy, PartialEq)]
struct CurationRunSignals {
    runs: Signal<Vec<CurationRun>>,
    selected_run: Signal<Option<String>>,
}

#[derive(Clone, Copy, PartialEq)]
struct CurationLifecycleSignals {
    loaded: Signal<bool>,
    command: Signal<Option<CurationPendingCommand>>,
    load_error: Signal<Option<String>>,
    curations: Signal<Vec<CurationSummary>>,
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
}

impl CurationController {
    pub(crate) fn use_controller() -> Self {
        let initial = use_hook(Self::fresh_curation);
        use_hook(|| Self {
            document: CurationDocumentSignals {
                baseline: Signal::new(initial.clone()),
                draft: Signal::new(initial),
                conflict: Signal::new(false),
            },
            runs: CurationRunSignals {
                runs: Signal::new(Vec::new()),
                selected_run: Signal::new(None),
            },
            lifecycle: CurationLifecycleSignals {
                loaded: Signal::new(false),
                command: Signal::new(None),
                load_error: Signal::new(None),
                curations: Signal::new(Vec::new()),
            },
            view: CurationViewSignals {
                active_panel: Signal::new(None),
                selected_episode: Signal::new(None),
                hovered_rule: Signal::new(None),
                filter: Signal::new(SheetFilter::All),
                lens: Signal::new(None),
                order: Signal::new(SheetOrder::Recorded),
            },
        })
    }

    pub(crate) fn spec(&self) -> CurationSpec {
        self.document.draft.read().clone()
    }
    /// Every recipe edit goes through here so the draft stays one value.
    pub(crate) fn edit(self, change: impl FnOnce(&mut CurationSpec)) {
        let mut draft = self.document.draft;
        change(&mut draft.write());
    }
    pub(crate) fn validation(&self) -> CurationValidationReport {
        self.document.draft.read().validate()
    }
    pub(crate) fn set_name(self, name: String) {
        self.edit(|spec| spec.set_name(name));
    }

    /// The run the sheet shows: the selected one, else the latest for this
    /// recipe, with staleness judged against the current draft.
    pub(crate) fn snapshot(&self) -> CurationRunSnapshot {
        let draft = self.document.draft.read();
        match self.visible_run() {
            Some(run) => {
                let mut snapshot = run.snapshot().clone();
                snapshot.set_result_stale(!draft.accepts_output_from(run.curation()));
                snapshot
            }
            None => CurationRunSnapshot::idle_for(&draft),
        }
    }
    pub(crate) fn outcome(&self) -> Option<CurationOutcome> {
        self.visible_run()
            .and_then(|run| run.snapshot().outcome().cloned())
    }
    /// The recipe that produced the visible verdicts.
    pub(crate) fn judged_spec(&self) -> Option<CurationSpec> {
        self.visible_run().map(|run| run.curation().clone())
    }
    fn visible_run(&self) -> Option<CurationRun> {
        let runs = self.curation_runs();
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
    pub(crate) fn curation_runs(&self) -> Vec<CurationRun> {
        let id = self.document.draft.read().id().to_owned();
        let mut runs = self
            .runs
            .runs
            .read()
            .iter()
            .filter(|run| run.curation().id() == id)
            .cloned()
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.snapshot().started_ms());
        runs
    }
    pub(crate) fn select_run(mut self, id: String) {
        if self
            .curation_runs()
            .iter()
            .any(|run| run.snapshot().run_id() == Some(id.as_str()))
        {
            self.runs.selected_run.set(Some(id));
            self.view.selected_episode.set(None);
        }
    }

    pub(crate) fn loaded(&self) -> bool {
        *self.lifecycle.loaded.read()
    }
    pub(crate) fn load_error(&self) -> Option<String> {
        self.lifecycle.load_error.read().clone()
    }
    pub(crate) fn command(&self) -> Option<CurationPendingCommand> {
        *self.lifecycle.command.read()
    }
    pub(crate) fn curations(&self) -> Vec<CurationSummary> {
        self.lifecycle.curations.read().clone()
    }
    pub(crate) fn revision_conflict(&self) -> bool {
        *self.document.conflict.read()
    }
    pub(crate) fn unsaved_changes(&self) -> bool {
        *self.document.draft.read() != *self.document.baseline.read()
    }

    pub(crate) fn active_panel(&self) -> Option<PanelId> {
        self.view.active_panel.read().clone()
    }
    pub(crate) fn activate_panel(mut self, id: PanelId) {
        self.view.active_panel.set(Some(id));
    }
    pub(crate) fn selected_episode(&self) -> Option<String> {
        self.view.selected_episode.read().clone()
    }
    /// The episode whose evidence the side sheet shows; `None` closes it.
    pub(crate) fn select_episode(mut self, id: Option<String>) {
        if *self.view.selected_episode.peek() != id {
            self.view.selected_episode.set(id);
        }
    }
    /// The measurement the tiles print: the chosen one while the judged
    /// recipe still declares it, else the first declared measurement.
    pub(crate) fn lens(&self, judged: &CurationSpec) -> Option<CurationFact> {
        let chosen = self.view.lens.read();
        chosen
            .as_deref()
            .and_then(|key| judged.facts().iter().find(|fact| fact.key() == key))
            .or_else(|| judged.facts().first())
            .cloned()
    }
    pub(crate) fn set_lens(mut self, key: String) {
        self.view.lens.set(Some(key));
    }
    pub(crate) fn order(&self) -> SheetOrder {
        *self.view.order.read()
    }
    pub(crate) fn cycle_order(mut self) {
        let next = self.view.order.peek().next();
        self.view.order.set(next);
    }
    pub(crate) fn hovered_rule(&self) -> Option<String> {
        self.view.hovered_rule.read().clone()
    }
    pub(crate) fn set_hovered_rule(mut self, id: Option<String>) {
        if *self.view.hovered_rule.peek() != id {
            self.view.hovered_rule.set(id);
        }
    }
    pub(crate) fn filter(&self) -> SheetFilter {
        *self.view.filter.read()
    }
    pub(crate) fn set_filter(mut self, filter: SheetFilter) {
        self.view.filter.set(filter);
    }

    pub(crate) fn run_disabled_reason(&self) -> Option<String> {
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved curation: {error}"));
        }
        if !self.loaded() {
            return Some("Loading the saved curation…".to_owned());
        }
        if self.command().is_some() {
            return Some("A curation command is in progress.".to_owned());
        }
        if self
            .runs
            .runs
            .read()
            .iter()
            .any(|run| run.snapshot().phase().active())
        {
            return Some("A curation is already running.".to_owned());
        }
        None
    }
    /// What the recipe still lacks before it can run. Run stays enabled so
    /// the answer is one click away instead of hidden in a tooltip.
    pub(crate) fn run_problem(&self) -> Option<String> {
        self.validation()
            .errors()
            .first()
            .map(|issue| format!("{} {}", issue.message(), issue.recovery()))
    }
    pub(crate) fn save_disabled_reason(&self) -> Option<String> {
        if !self.loaded() {
            return Some("Loading curation…".into());
        }
        if let Some(error) = self.load_error() {
            return Some(format!("Could not load the saved curation: {error}"));
        }
        if self.revision_conflict() {
            return Some(
                "The curation changed in another session. Reload it before editing again."
                    .to_owned(),
            );
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
    pub(crate) fn selection_reason(&self) -> Option<String> {
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

    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.document.draft.origin_scope(), task);
    }
    fn fresh_curation() -> CurationSpec {
        CurationSpec::new(
            format!(
                "curation-{}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * u64::MAX as f64) as u64
            ),
            DEFAULT_CURATION_NAME,
        )
    }
    fn install(mut self, curation: CurationSpec) {
        self.document.draft.set(curation.clone());
        self.document.baseline.set(curation);
        self.document.conflict.set(false);
        self.runs.selected_run.set(None);
        self.view.selected_episode.set(None);
        self.view.hovered_rule.set(None);
        self.view.filter.set(SheetFilter::All);
        self.view.lens.set(None);
        self.view.order.set(SheetOrder::Recorded);
        self.lifecycle.loaded.set(true);
        self.lifecycle.load_error.set(None);
    }

    pub(crate) fn save(mut self, studio: StudioProjectState) {
        if self.save_disabled_reason().is_some() {
            return;
        }
        let submitted = self.spec();
        self.lifecycle
            .command
            .set(Some(CurationPendingCommand::Saving));
        self.spawn(async move {
            let client = CurationClient::current();
            match client.save(&submitted).await {
                Ok(saved) => {
                    // A save acknowledges its input; edits made meanwhile stay local.
                    if self.spec().id() == saved.id() {
                        self.document
                            .draft
                            .write()
                            .set_revision(saved.revision());
                        self.document.baseline.set(saved);
                        self.document.conflict.set(false);
                    }
                    if let Ok(curations) = client.list().await {
                        self.lifecycle.curations.set(curations);
                    }
                }
                Err(error) => {
                    self.document.conflict.set(
                        error.code()
                            == spiral_robotics_common::command_failure::CommandFailureCode::Conflict,
                    );
                    studio.report_from(MessageSource::Curation, error.to_string());
                }
            }
            self.lifecycle.command.set(None);
        });
    }
    pub(crate) fn discard(self) {
        if self.command().is_some() {
            return;
        }
        let baseline = self.document.baseline.peek().clone();
        self.install(baseline);
    }
    pub(crate) fn run(mut self, studio: StudioProjectState) {
        if self.run_disabled_reason().is_some() {
            return;
        }
        if let Some(problem) = self.run_problem() {
            studio.report_warning_from(MessageSource::Curation, problem);
            return;
        }
        let curation = self.spec();
        self.lifecycle
            .command
            .set(Some(CurationPendingCommand::Starting));
        self.spawn(async move {
            match CurationClient::current().start(&curation).await {
                Ok(run) => {
                    self.runs.selected_run.set(None);
                    self.view.selected_episode.set(None);
                    self.runs.runs.accept_runs(vec![run]);
                }
                Err(error) => studio.report_from(MessageSource::Curation, error.to_string()),
            }
            self.lifecycle.command.set(None);
            QueryRefresh::current().invalidate(&[QueryDomain::Curation]);
        });
    }
    pub(crate) fn select_curation(self, id: Option<String>) {
        if self.selection_reason().is_some() {
            return;
        }
        self.open_document(id);
    }
    pub(crate) fn reload_current(mut self) {
        self.lifecycle.load_error.set(None);
        let id = self.spec().id().to_owned();
        let known = self.curations().iter().any(|curation| curation.id() == id);
        if !self.loaded() {
            self.load_initial();
        } else if known {
            self.open_document(Some(id));
        } else {
            self.install(Self::fresh_curation());
        }
    }
    fn open_document(mut self, id: Option<String>) {
        let Some(id) = id else {
            self.install(Self::fresh_curation());
            return;
        };
        self.lifecycle
            .command
            .set(Some(CurationPendingCommand::Selecting));
        self.spawn(async move {
            match CurationClient::current().load(&id).await {
                Ok(curation) => self.install(curation),
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
    }
    fn load_initial(mut self) {
        self.lifecycle
            .command
            .set(Some(CurationPendingCommand::Selecting));
        self.spawn(async move {
            let client = CurationClient::current();
            let result = async {
                let curations = client.list().await?;
                let curation = match curations.first() {
                    Some(summary) => client.load(summary.id()).await?,
                    None => Self::fresh_curation(),
                };
                Ok::<_, crate::client::transport::CommandError>((curations, curation))
            }
            .await;
            match result {
                Ok((curations, curation)) => {
                    self.lifecycle.curations.set(curations);
                    self.install(curation);
                }
                Err(error) => self.lifecycle.load_error.set(Some(error.to_string())),
            }
            self.lifecycle.command.set(None);
        });
    }

    pub(crate) fn use_polling(self, is_active: ReadSignal<bool>) {
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
        let active = use_memo(move || {
            self.runs
                .runs
                .read()
                .iter()
                .any(|run| run.snapshot().phase().active())
                || self.command() == Some(CurationPendingCommand::Starting)
        });
        let runs = QueryPolicy::new(QueryDomain::Curation, 500)
            .with_activity(active.into())
            .use_when(
                move || is_active() && sessions.studio_available(),
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
                        let client = CurationClient::current();
                        match active {
                            Some(id) => client.run(&id).await.map(|run| vec![run]),
                            None => client.runs().await,
                        }
                    }
                },
            );
        self.runs.runs.observe(runs);
    }

    pub(crate) fn editor_status(&self) -> Option<CurationEditorStatus> {
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
    pub(crate) fn new(curation: CurationSpec, snapshot: CurationRunSnapshot) -> Self {
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
        let idle = CurationEditorStatus::new(spec.clone(), CurationRunSnapshot::idle_for(&spec));
        assert_eq!(idle.dataset_label(), "dataset_x");
        assert_eq!(idle.phase_label(), "");
        assert_eq!(idle.verdict_label(), "Bright");
        let mut snapshot = CurationRunSnapshot::started(&spec, "run");
        snapshot.set_phase(RunPhase::Completed);
        snapshot.set_outcome(Some(CurationOutcome::new(
            spiral_robotics_common::curation::run::CurationTotals::new(40, 31, (40, false)),
            Vec::new(),
        )));
        let complete = CurationEditorStatus::new(spec.clone(), snapshot);
        assert_eq!(complete.phase_label(), "");
        assert_eq!(complete.verdict_label(), "31 of 40 kept");
        let mut running = CurationRunSnapshot::started(&spec, "run");
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
