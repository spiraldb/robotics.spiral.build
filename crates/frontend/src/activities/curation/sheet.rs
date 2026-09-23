//! The contact sheet: one tile per episode, kept tiles in full ink, excluded
//! tiles dimmed under the rule that removed them. Touch a rule's mark in the
//! recipe and the tiles it struck light up; open a tile and its evidence
//! docks beside the sheet. The pipeline strip above selects which stage the
//! sheet lays out.

use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationSpec;
use spiral_robotics_common::curation::facts::{CurationFact, FactKind};
use spiral_robotics_common::curation::run::{CurationOutcome, CurationVerdict};
use spiral_robotics_common::runs::RunPhase;

use super::client::CurationClient;
use super::episode::EpisodeSheet;
use super::pipeline::{
    FilterHistogram, MeasuredEpisode as _, Pipeline, PipelinePending, PipelineSelection,
    PipelineStage, StageKind, StageMark,
};
use super::state::{CurationController, SheetFilter, SheetOrder};
use crate::activities::episode::{Episode, EpisodeFactsPresentation as _, EpisodeLabels};
use crate::components::document_bar::ExecutedSql;
use crate::components::icons::{CurationIcon, ResultNodeIcon, SpinnerIcon};
use crate::components::run_strip::{RunChoice, RunDuration, RunStrip};
use crate::shell::state::StudioProjectState;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::SheetPosition;
use spiral_robotics_ui::{
    ActionPurpose, ArrowDownIcon, ArrowUpIcon, BrowserFocus, Button, ButtonVariant, Chip, ChipTone,
    Disclosure, EmptyState, ModeButton, ModeGroup, NoticeTone, Select,
};

/// Focus lands here when a tile opens its evidence.
pub(super) const EPISODE_HEADING_ID: &str = "curation-episode-heading";

/// The six identity washes, dealt to rules in recipe order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RuleTone(usize);

impl RuleTone {
    /// Dealt so the first rules sit far from role blue: a lit ring must never
    /// be mistaken for the selected tile's ring. Blue comes last.
    const DEAL: [usize; 6] = [2, 4, 3, 5, 6, 1];
    const fn at(index: usize) -> Self {
        Self(Self::DEAL[index % 6])
    }
    pub(super) fn css(self) -> String {
        format!("var(--tone-category-{})", self.0)
    }
    /// The tone of a rule by identity within the judged recipe.
    pub(super) fn of(spec: &CurationSpec, rule_id: &str) -> Self {
        Self::at(
            spec.rules()
                .iter()
                .position(|rule| rule.id() == rule_id)
                .unwrap_or(0),
        )
    }
}

#[component]
pub(super) fn SheetPanel(controller: CurationController) -> Element {
    let studio = StudioProjectState::current();
    let spec = controller.spec();
    let snapshot = controller.snapshot();
    // A step still being written must not take the measurements away from
    // the rest of the recipe: the preview judges every complete step and the
    // incomplete one's card says so.
    let eligibility = spec.source().map(|_| EpisodeEligibility(spec.measurable()));
    let mut metadata = use_resource(use_reactive(
        (&eligibility,),
        move |(eligibility,)| async move {
            match eligibility {
                Some(eligibility) => CurationClient::current()
                    .preview(&eligibility.0)
                    .await
                    .map(Some)
                    .map_err(|error| error.to_string()),
                None => Ok(None),
            }
        },
    ));
    let preview = if *metadata.state().read() == UseResourceState::Pending {
        EligibilityState::Pending
    } else {
        match metadata.read().as_ref() {
            Some(Ok(Some(outcome))) => EligibilityState::Ready(outcome.clone()),
            Some(Err(error)) => EligibilityState::Failed(error.clone()),
            _ => EligibilityState::Empty,
        }
    };
    let run_outcome = controller.outcome();
    let run_spec = controller.judged_spec();
    let current_run = !snapshot.result_stale()
        && run_spec
            .as_ref()
            .is_some_and(|judged| spec.accepts_output_from(judged));
    let outcome = run_outcome
        .clone()
        .filter(|_| current_run)
        .or_else(|| preview.outcome().cloned());
    let judged = outcome.as_ref().map(|_| {
        if current_run {
            run_spec.clone().unwrap_or_else(|| spec.clone())
        } else {
            spec.clone()
        }
    });
    let runs = controller.curation_runs();
    let run_choices = runs
        .iter()
        .enumerate()
        .map(|(place, run)| {
            RunChoice::new(
                run.snapshot().run_id().unwrap_or_default(),
                (place + 1, runs.len()),
                run.snapshot().phase(),
            )
        })
        .collect::<Vec<_>>();
    let shown_run = run_choices
        .iter()
        .find(|choice| Some(choice.id()) == snapshot.run_id())
        .cloned();
    let filter = controller.filter();
    let order = controller.order();
    let selected = controller.selected_episode();
    let phase = snapshot.phase();
    let lens = judged.as_ref().and_then(|judged| controller.lens(judged));
    let selection = controller.pipeline_selection();
    let stages = PipelineStage::all(&spec, outcome.as_ref());
    let stage = stages
        .iter()
        .find(|stage| stage.selection == selection)
        .cloned();
    let pending = if phase.active() {
        PipelinePending::Running
    } else if preview == EligibilityState::Pending {
        PipelinePending::Measuring
    } else if run_outcome.is_some() && !current_run {
        PipelinePending::Stale
    } else {
        PipelinePending::Unmeasured
    };
    let roster = Roster::new(outcome.as_ref(), filter, lens.as_ref(), order);
    let roster = match (judged.as_ref(), outcome.as_ref()) {
        (Some(spec), Some(_)) => roster.for_stage(&selection, spec),
        _ => roster,
    };
    let transform_stage = stage
        .as_ref()
        .filter(|stage| {
            matches!(
                stage.kind,
                StageKind::Model | StageKind::Media | StageKind::Trim
            )
        })
        .and_then(|stage| stage.selection.step_id().map(str::to_owned));
    let filter_rule = stage
        .as_ref()
        .filter(|stage| stage.kind == StageKind::Filter)
        .and_then(|stage| stage.selection.step_id().map(str::to_owned));
    let focus = SheetFocus::new(controller, lens.clone(), filter_rule.clone());
    let output_selected = selection == PipelineSelection::Output;
    // The curated output always shows one episode's evidence beside the kept
    // tiles: the first one until another is chosen, and never none.
    let output_roster = output_selected.then(|| roster.clone());
    use_effect(use_reactive(&output_roster, move |output_roster| {
        let Some(roster) = output_roster else {
            return;
        };
        if roster.len() == 0 {
            return;
        }
        let selected = controller.selected_episode();
        if selected
            .as_ref()
            .and_then(|id| roster.position_of(id))
            .is_none()
        {
            controller.select_episode(
                roster
                    .get(0)
                    .map(|tile| tile.verdict().episode().id().clone()),
            );
        }
    }));
    let select = move |id: EpisodeId| {
        if !output_selected && controller.selected_episode().as_ref() == Some(&id) {
            controller.select_episode(None);
        } else {
            controller.select_episode(Some(id));
            BrowserFocus::after_render(EPISODE_HEADING_ID.to_owned());
        }
    };
    let open = selected
        .as_ref()
        .and_then(|id| roster.position_of(id))
        .map(|index| SheetPosition::new(index, roster.len()));
    rsx! {
        section { class: "curation-sheet", "aria-label": "Curation sheet",
            if let Some(current) = shown_run {
                RunStrip {
                    id: "curation-result-run",
                    runs: run_choices,
                    current,
                    on_select: move |id| controller.select_run(id),
                    facts: rsx! {
                        if let Some(outcome) = run_outcome.as_ref().filter(|_| current_run) {
                            span { class: "run-duration", title: "Spiral judging time", "{RunDuration::millis(outcome.elapsed_ms())}" }
                            if outcome.totals().truncated() {
                                Chip { tone: ChipTone::Caution, title: "The sheet lists the first {outcome.totals().listed()} episodes; the counts cover every episode.", "Sheet truncated" }
                            }
                        }
                        if phase.active() {
                            span { class: "run-elapsed", role: "status", "Reading episodes…" }
                        }
                        if let Some(sql) = snapshot.sql() {
                            ExecutedSql { sql: sql.to_owned() }
                        }
                    },
                }
            }
            if snapshot.result_stale() && preview.outcome().is_none() && run_outcome.is_some() {
                div { class: "results-stale", role: "status",
                    strong { "Previous run" }
                    span { "The recipe has changed since these verdicts. Run again." }
                }
            }
            if let Some(relation) = snapshot.output_relation() {
                Disclosure { class: "curation-output-note", summary: rsx! { "Saved selection and derived outputs" },
                    p { "Episode selection: " code { "{relation}" } " · filter by kept" }
                    for derived in snapshot.derived_relations() { p { "Derived output: " code { "{derived}" } } }
                    p { "The exact recipe is stored with every verdict. Source recordings remain unchanged." }
                }
            }
            div { class: "curation-sheet-stage",
                Pipeline { controller, stages: stages.clone(), pending }
                div { class: "curation-stage-area", "data-split": if output_selected { "half" } else { "dock" },
                    div { class: "curation-sheet-body",
                        if let (Some(stage_id), Some(stage)) = (transform_stage.as_ref(), stage.as_ref()) {
                            TransformStagePreview { controller, spec: spec.clone(), stage: stage.clone(), stage_id: stage_id.clone(), preview: preview.clone(), on_retry: move |_| metadata.restart() }
                        } else {
                            if let (Some(stage), Some(judged), Some(outcome)) = (stage.as_ref(), judged.as_ref(), outcome.as_ref()) {
                                StageHeading { stage: stage.clone(),
                                    if stage.kind == StageKind::Source {
                                        ModeGroup { label: "Show".to_owned(), class: "curation-sheet-filter".to_owned(),
                                            for (option, count) in [
                                                (SheetFilter::All, outcome.totals().episodes()),
                                                (SheetFilter::Kept, outcome.totals().kept()),
                                                (SheetFilter::Excluded, outcome.totals().excluded()),
                                            ] {
                                                ModeButton { purpose: ActionPurpose::Select, checked: filter == option,
                                                    onclick: move |_| controller.set_filter(option),
                                                    "{option.label()} " strong { "{count}" } }
                                            }
                                        }
                                    }
                                    if !judged.facts().is_empty() {
                                        SheetLens { controller, judged: judged.clone(), lens: lens.clone(), order }
                                    }
                                }
                                if let (Some(rule_id), Some(tone)) = (filter_rule.as_ref(), stage.tone) {
                                    FilterHistogram { spec: judged.clone(), outcome: outcome.clone(), rule_id: rule_id.clone(), tone }
                                }
                            }
                            if !controller.loaded() && controller.load_error().is_none() {
                                EmptyState { icon: rsx! { SpinnerIcon {} }, title: "Loading curation…" }
                            } else if let Some(error) = controller.load_error() {
                                EmptyState { tone: NoticeTone::Danger, title: "Could not load the saved curation",
                                    p { "{error}" }
                                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, onclick: move |_| controller.reload_current(), "Try again" }
                                }
                            } else if let Some(judged) = judged.as_ref().filter(|_| outcome.is_some()) {
                                if roster.len() == 0 && selection != PipelineSelection::Source {
                                    p { class: "curation-section-note curation-stage-empty",
                                        if selection == PipelineSelection::Output { "No episode is kept by the current recipe." } else { "This filter excluded no episode." } }
                                } else {
                                    TileGrid {
                                        roster: roster.clone(), judged: judged.clone(),
                                        focus: focus.clone(), filter, on_select: select,
                                    }
                                }
                            } else if phase.active() || preview == EligibilityState::Pending {
                                SheetSkeleton {}
                            } else if let EligibilityState::Failed(error) = &preview {
                                EmptyState { tone: NoticeTone::Danger, title: "Could not read episode metadata",
                                    p { class: "curation-error-detail", "{error}" }
                                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                                        onclick: move |_| metadata.restart(), "Try again" }
                                }
                            } else if phase == RunPhase::Failed {
                                EmptyState { tone: NoticeTone::Danger, title: "Run failed",
                                    p { class: "curation-error-detail", "{snapshot.error().unwrap_or(\"Spiral did not say why.\")}" }
                                    if controller.run_disabled_reason().is_none() {
                                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, onclick: move |_| controller.run(studio), "Run again" }
                                    }
                                }
                            } else if spec.source().is_none() {
                                EmptyState { icon: rsx! { CurationIcon {} }, title: "Name a dataset",
                                    p { "Every one of its episodes becomes a tile here, kept or excluded, with the rule that decided." }
                                }
                            } else {
                                EmptyState { icon: rsx! { ResultNodeIcon {} }, title: "Nothing judged yet",
                                    p { "Run the recipe to lay the episodes out." }
                                }
                            }
                        }
                    }
                    if output_selected {
                        if let (Some(position), Some(judged)) = (open, judged.as_ref()) {
                            if let Some(tile) = roster.get(position.index()) {
                                OutputDetail {
                                    key: "{tile.verdict().episode().id()}",
                                    tile: tile.clone(),
                                    judged: judged.clone(),
                                    position,
                                    on_step: {
                                        let roster = roster.clone();
                                        move |delta: isize| {
                                            if let Some(next) = position.stepped(delta).and_then(|next| roster.get(next.index())) {
                                                controller.select_episode(Some(next.verdict().episode().id().clone()));
                                            }
                                        }
                                    },
                                }
                            }
                        }
                    } else if transform_stage.is_none() {
                        if let (Some(position), Some(judged)) = (open, judged.as_ref()) {
                            if let Some(tile) = roster.get(position.index()) {
                                EpisodeSheet {
                                    key: "{tile.verdict().episode().id()}",
                                    tile: tile.clone(),
                                    judged: judged.clone(),
                                    selected_stage: filter_rule.clone(),
                                    position,
                                    on_step: {
                                        let roster = roster.clone();
                                        move |delta: isize| {
                                            if let Some(next) = position.stepped(delta).and_then(|next| roster.get(next.index())) {
                                                controller.select_episode(Some(next.verdict().episode().id().clone()));
                                                BrowserFocus::after_render(EPISODE_HEADING_ID.to_owned());
                                            }
                                        }
                                    },
                                    on_close: {
                                        let id = tile.verdict().episode().id().clone();
                                        move |_| {
                                            controller.select_episode(None);
                                            BrowserFocus::after_render(SheetTile::element_id(&id));
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The curated output reads as a split: the kept tiles at the left, the
/// selected episode's evidence at the right, always open so the stage never
/// shows an empty half. ↑ and ↓ step; there is nothing to close.
#[component]
fn OutputDetail(
    tile: SheetTile,
    judged: CurationSpec,
    position: SheetPosition,
    on_step: EventHandler<isize>,
) -> Element {
    rsx! {
        section { class: "curation-output-detail", "aria-label": "Curated episode details",
            onkeydown: move |event: KeyboardEvent| match event.key() {
                Key::ArrowUp => { event.prevent_default(); on_step.call(-1); }
                Key::ArrowDown => { event.prevent_default(); on_step.call(1); }
                _ => {}
            },
            header { class: "ui-side-sheet-heading",
                h3 {
                    "{Episode::name_of(tile.verdict().episode().id())}"
                    span { class: "ui-side-sheet-count", " · {position.index() + 1} of {position.count()} kept" }
                }
                div { class: "ui-side-sheet-nav", role: "group", "aria-label": "Step between kept episodes",
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                        aria_label: "Previous episode", title: "Previous episode · ↑",
                        disabled: position.stepped(-1).is_none(),
                        onclick: move |_| on_step.call(-1), ArrowUpIcon {} }
                    Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                        aria_label: "Next episode", title: "Next episode · ↓",
                        disabled: position.stepped(1).is_none(),
                        onclick: move |_| on_step.call(1), ArrowDownIcon {} }
                }
            }
            EpisodeSheet {
                tile, judged, inline: true, position,
                on_step: move |delta: isize| on_step.call(delta),
                on_close: move |_| {},
            }
        }
    }
}

/// One line under the pipeline that names the selected stage the way its
/// card does, then holds the controls for the list beneath it.
#[component]
fn StageHeading(stage: PipelineStage, children: Element) -> Element {
    rsx! {
        div { class: "curation-stage-heading",
            StageMark { kind: stage.kind, tone: stage.tone }
            h3 { class: "curation-stage-title", "{stage.title}" }
            span { class: "curation-stage-summary", title: "{stage.summary}", "{stage.summary}" }
            div { class: "curation-stage-tools", {children} }
        }
    }
}

trait MeasurableRecipe {
    /// This recipe with every incomplete step left out, so the preview can run
    /// while a new rule waits for its value.
    fn measurable(&self) -> Self;
}

impl MeasurableRecipe for CurationSpec {
    fn measurable(&self) -> Self {
        let catalog = self.catalog();
        let mut measurable = self.clone();
        for rule in self.rules() {
            if rule.condition().validate(&catalog).is_err() {
                measurable.remove_rule(rule.id());
            }
        }
        for transform in self.transforms() {
            if transform.validate(&catalog).is_err() {
                measurable.remove_transform(transform.id());
            }
        }
        measurable
    }
}

/// Model parameters do not change episode eligibility. Keeping this query key
/// independent of them preserves the mounted viewer and its frame scores when
/// the user adjusts a chunk threshold or camera transform.
#[derive(Clone)]
struct EpisodeEligibility(CurationSpec);

impl PartialEq for EpisodeEligibility {
    fn eq(&self, other: &Self) -> bool {
        let (left, right) = (&self.0, &other.0);
        left.source() == right.source()
            && left.facts() == right.facts()
            && left.rules().len() == right.rules().len()
            && left
                .rules()
                .iter()
                .zip(right.rules())
                .all(|(left, right)| left.judges_like(right))
            && left.transforms().len() == right.transforms().len()
            && left
                .transforms()
                .iter()
                .zip(right.transforms())
                .all(|(left, right)| {
                    left.id() == right.id()
                        && left.order() == right.order()
                        && left.condition() == right.condition()
                })
    }
}

#[derive(Clone, PartialEq)]
enum EligibilityState {
    Empty,
    Pending,
    Ready(CurationOutcome),
    Failed(String),
}

impl EligibilityState {
    fn outcome(&self) -> Option<&CurationOutcome> {
        match self {
            Self::Ready(outcome) => Some(outcome),
            _ => None,
        }
    }
}

/// Read-only upstream eligibility supplies the episode picker before any run.
/// Model execution belongs to the selected episode's evidence component.
#[component]
fn TransformStagePreview(
    controller: CurationController,
    spec: CurationSpec,
    stage: PipelineStage,
    stage_id: String,
    preview: EligibilityState,
    on_retry: EventHandler<()>,
) -> Element {
    let roster = use_memo(use_reactive(
        (&spec, &stage_id, &preview),
        move |(spec, stage_id, preview)| {
            Roster::new(
                preview.outcome(),
                SheetFilter::All,
                None,
                SheetOrder::Recorded,
            )
            .for_stage(&PipelineSelection::Step(stage_id), &spec)
        },
    ));
    use_effect(move || {
        let roster = roster.read();
        if roster.len() == 0 {
            return;
        }
        let selected = controller.selected_episode();
        if selected
            .as_ref()
            .and_then(|id| roster.position_of(id))
            .is_none()
        {
            controller.select_episode(
                roster
                    .get(0)
                    .map(|tile| tile.verdict().episode().id().clone()),
            );
        }
    });
    let roster = roster.read();
    let selected = controller.selected_episode();
    let position = selected
        .as_ref()
        .and_then(|id| roster.position_of(id))
        .map(|index| SheetPosition::new(index, roster.len()));
    let step = {
        let roster = roster.clone();
        move |delta: isize| {
            if let Some(next) = position
                .and_then(|position| position.stepped(delta))
                .and_then(|next| roster.get(next.index()))
            {
                controller.select_episode(Some(next.verdict().episode().id().clone()));
            }
        }
    };
    let eligible = roster.len();
    rsx! {
        section { class: "curation-stage-preview", "aria-label": "Transform episode review",
            StageHeading { stage: stage.clone(),
                if let Some(position) = position {
                    div { class: "curation-episode-stepper", role: "group", "aria-label": "Step between eligible episodes",
                        // ↑ and ↓ step while a stepper button has focus; the
                        // select beside them keeps its own arrow keys.
                        span { class: "curation-episode-steps",
                            onkeydown: {
                                let step = step.clone();
                                move |event: KeyboardEvent| match event.key() {
                                    Key::ArrowUp => { event.prevent_default(); step(-1); }
                                    Key::ArrowDown => { event.prevent_default(); step(1); }
                                    _ => {}
                                }
                            },
                            Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                                aria_label: "Previous episode", title: "Previous episode · ↑",
                                disabled: position.stepped(-1).is_none(),
                                onclick: { let step = step.clone(); move |_| step(-1) }, ArrowUpIcon {} }
                            Button { purpose: ActionPurpose::Navigate, variant: ButtonVariant::Icon,
                                aria_label: "Next episode", title: "Next episode · ↓",
                                disabled: position.stepped(1).is_none(),
                                onclick: { let step = step.clone(); move |_| step(1) }, ArrowDownIcon {} }
                        }
                        label { r#for: "curation-preview-episode", class: "sr-only", "Episode" }
                        Select { id: "curation-preview-episode", class: "run-select curation-episode-select",
                            title: selected.as_ref().map(EpisodeId::to_string).unwrap_or_default(),
                            value: selected.as_ref().map(EpisodeId::to_string).unwrap_or_default(),
                            onchange: move |value: String| {
                                if let Ok(id) = EpisodeId::try_from(value) {
                                    controller.select_episode(Some(id));
                                }
                            },
                            for tile in &roster.tiles {
                                option { key: "{tile.verdict().episode().id()}", value: tile.verdict().episode().id().as_str(),
                                    selected: selected.as_ref() == Some(tile.verdict().episode().id()),
                                    "…{tile.verdict().episode().id().short()} · {tile.label()} · {tile.verdict().episode().duration_label()}"
                                }
                            }
                        }
                    }
                }
            }
            if preview == EligibilityState::Pending {
                div { class: "curation-preview-status", role: "status", SpinnerIcon {} "Reading eligible episodes…" }
            } else if let EligibilityState::Failed(error) = &preview {
                div { class: "curation-preview-status", role: "alert",
                    p { "Could not read eligible episodes: {error}" }
                    Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                        onclick: move |_| on_retry.call(()), "Try again" }
                }
            } else if eligible == 0 {
                p { class: "curation-section-note curation-stage-empty", "No episode reaches this transform with the current recipe." }
            } else {
                if let Some(outcome) = preview.outcome() {
                    if outcome.totals().truncated() {
                        p { class: "curation-section-note", "Showing the first {outcome.totals().listed()} source episodes; later eligible episodes are not listed." }
                    }
                }
                if let Some(position) = position {
                    if let Some(tile) = roster.get(position.index()) {
                        EpisodeSheet {
                            key: "{tile.verdict().episode().id()}",
                            tile: tile.clone(), judged: spec.clone(), selected_stage: Some(stage_id.clone()),
                            inline: true,
                            position,
                            on_step: { let step = step.clone(); move |delta: isize| step(delta) },
                            // Inline evidence has no close control; the card is the way out.
                            on_close: move |_| {},
                        }
                    }
                }
            }
        }
    }
}

/// Which measurement the tiles print, and how the sheet is ordered by it.
#[component]
fn SheetLens(
    controller: CurationController,
    judged: CurationSpec,
    lens: Option<CurationFact>,
    order: SheetOrder,
) -> Element {
    rsx! {
        span { class: "curation-lens",
            label { r#for: "curation-lens", class: "curation-lens-word", "Tiles" }
            Select { id: "curation-lens", class: "run-select", title: "The measurement every tile prints",
                value: lens.as_ref().map(|fact| fact.key().to_owned()).unwrap_or_default(),
                onchange: move |value| controller.set_lens(value),
                for fact in judged.facts() {
                    option { value: "{fact.key()}", selected: lens.as_ref().is_some_and(|lens| lens.key() == fact.key()), "{fact.label()}" }
                }
            }
            Button { purpose: ActionPurpose::Toggle, variant: ButtonVariant::Quiet, class: "curation-order",
                title: "Order the sheet by this measurement",
                onclick: move |_| controller.cycle_order(),
                "{order.label()}"
            }
        }
    }
}

/// The tiles on view, in the order the sheet lays them out.
#[derive(Clone, PartialEq)]
struct Roster {
    tiles: Vec<SheetTile>,
}

impl Roster {
    fn new(
        outcome: Option<&CurationOutcome>,
        filter: SheetFilter,
        lens: Option<&CurationFact>,
        order: SheetOrder,
    ) -> Self {
        let Some(outcome) = outcome else {
            return Self { tiles: Vec::new() };
        };
        let labels = EpisodeLabels::of(outcome.verdicts().iter().map(CurationVerdict::episode));
        let mut tiles = outcome
            .verdicts()
            .iter()
            .filter(|verdict| filter.admits(verdict.kept()))
            .map(|verdict| SheetTile {
                label: labels.label(verdict.episode()).head().to_owned(),
                verdict: verdict.clone(),
            })
            .collect::<Vec<_>>();
        if let Some(lens) = lens.filter(|_| order != SheetOrder::Recorded) {
            // Missing values sink to the end whichever way the sheet reads.
            tiles.sort_by(|left, right| {
                let left = FactValueText::new(left.verdict.fact(lens.key()), lens.kind());
                let right = FactValueText::new(right.verdict.fact(lens.key()), lens.kind());
                match (left.sort_key(), right.sort_key()) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (Some(left), Some(right)) => {
                        let ordering = left
                            .partial_cmp(&right)
                            .unwrap_or(std::cmp::Ordering::Equal);
                        if order == SheetOrder::Descending {
                            ordering.reverse()
                        } else {
                            ordering
                        }
                    }
                }
            });
        }
        Self { tiles }
    }
    fn for_stage(mut self, selection: &PipelineSelection, spec: &CurationSpec) -> Self {
        self.tiles
            .retain(|tile| selection.admits(tile.verdict(), spec));
        self
    }
    fn len(&self) -> usize {
        self.tiles.len()
    }
    fn get(&self, index: usize) -> Option<&SheetTile> {
        self.tiles.get(index)
    }
    fn position_of(&self, episode_id: &EpisodeId) -> Option<usize> {
        self.tiles
            .iter()
            .position(|tile| tile.verdict.episode().id() == episode_id)
    }
}

/// One episode as the sheet knows it: the words that tell it from its
/// neighbours, and its verdict.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SheetTile {
    label: String,
    verdict: CurationVerdict,
}

impl SheetTile {
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    pub(super) const fn verdict(&self) -> &CurationVerdict {
        &self.verdict
    }
    /// The tile's own button, where focus returns when its evidence closes.
    fn element_id(id: &EpisodeId) -> String {
        format!("curation-tile-{id}")
    }
}

#[component]
fn TileGrid(
    roster: Roster,
    judged: CurationSpec,
    focus: SheetFocus,
    filter: SheetFilter,
    on_select: EventHandler<EpisodeId>,
) -> Element {
    if roster.len() == 0 {
        return rsx! {
            EmptyState {
                p { match filter {
                    SheetFilter::Kept => "No episode was kept.",
                    SheetFilter::Excluded => "No episode was excluded.",
                    SheetFilter::All => "The dataset has no episodes.",
                } }
            }
        };
    }
    let catalog = judged.catalog();
    let tested = focus.tested(&judged, &catalog);
    let longest = roster
        .tiles
        .iter()
        .filter_map(|tile| tile.verdict.episode().duration_seconds())
        .fold(0.0_f64, f64::max);
    rsx! {
        ol { class: "curation-sheet-grid", "aria-label": "Episodes",
            for (position, tile) in roster.tiles.iter().enumerate() {
                EpisodeTile {
                    key: "{tile.verdict.episode().id()}",
                    tile: tile.clone(),
                    look: TileLook::of(tile, &judged, TileContext {
                        focus: &focus,
                        catalog: &catalog,
                        tested: tested.as_ref().map(|(key, label)| (key.as_str(), label.as_str())),
                        longest,
                    }),
                    position,
                    on_select,
                }
            }
        }
    }
}

/// What the sheet is looking at while it lays the tiles out: the measurement
/// every tile prints, the rule under the pointer, the episode whose evidence
/// is open, and the filter stage whose tiles these are.
#[derive(Clone, PartialEq)]
struct SheetFocus {
    lens: Option<CurationFact>,
    hovered_rule: Option<String>,
    open_episode: Option<EpisodeId>,
    stage_rule: Option<String>,
}

impl SheetFocus {
    fn new(
        controller: CurationController,
        lens: Option<CurationFact>,
        stage_rule: Option<String>,
    ) -> Self {
        Self {
            lens,
            hovered_rule: controller.hovered_rule(),
            open_episode: controller.selected_episode(),
            stage_rule,
        }
    }
    /// On a filter's own stage every tile answers that rule, so the tile prints
    /// the value the rule tested instead of repeating the rule's words.
    fn tested(
        &self,
        judged: &CurationSpec,
        catalog: &spiral_robotics_common::curation::facts::FactCatalog,
    ) -> Option<(String, String)> {
        self.stage_rule
            .as_deref()
            .and_then(|id| judged.rules().iter().find(|rule| rule.id() == id))
            .map(|rule| rule.condition().fact().to_owned())
            .filter(|key| key != "duration_s")
            .map(|key| {
                let label = catalog.label(&key);
                (key, label)
            })
    }
}

/// What every tile is judged against when it is drawn.
#[derive(Clone, Copy)]
struct TileContext<'a> {
    focus: &'a SheetFocus,
    catalog: &'a spiral_robotics_common::curation::facts::FactCatalog,
    /// The fact the open filter tests, printed on every tile in its place.
    tested: Option<(&'a str, &'a str)>,
    longest: f64,
}

/// How a tile answers the rule under the pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TileHighlight {
    Resting,
    Lit,
    Faded,
}

impl TileHighlight {
    fn for_rule(rule: Option<&str>, verdict: &CurationVerdict) -> Self {
        match rule {
            None => Self::Resting,
            Some(rule) if verdict.excluded_by().iter().any(|fired| fired == rule) => Self::Lit,
            Some(_) => Self::Faded,
        }
    }
    const fn attribute(self) -> &'static str {
        match self {
            Self::Resting => "resting",
            Self::Lit => "lit",
            Self::Faded => "faded",
        }
    }
}

/// The measurement a tile prints: the compact value on the tile, the label
/// and exact value in the tooltip.
#[derive(Clone, Debug, PartialEq)]
struct TileFact {
    label: String,
    value: FactValueText,
}

/// One reason chip: the rule's words in the rule's tint.
#[derive(Clone, Debug, PartialEq)]
struct Reason {
    text: String,
    tone: RuleTone,
}

/// Everything a tile draws beyond its identity, resolved once per render.
#[derive(Clone, Debug, PartialEq)]
struct TileLook {
    reasons: Vec<Reason>,
    shown_reasons: Vec<Reason>,
    fact: Option<TileFact>,
    facts: Vec<TileFact>,
    fill: f64,
    highlight: TileHighlight,
    selected: bool,
}

impl TileLook {
    fn of(tile: &SheetTile, judged: &CurationSpec, context: TileContext<'_>) -> Self {
        let verdict = &tile.verdict;
        let reasons: Vec<Reason> = verdict
            .excluded_by()
            .iter()
            .map(|rule_id| Reason {
                text: judged
                    .rules()
                    .iter()
                    .find(|rule| rule.id() == rule_id)
                    .map_or_else(|| rule_id.to_owned(), |rule| rule.reason(context.catalog)),
                tone: RuleTone::of(judged, rule_id),
            })
            .collect();
        // On a filter's own stage the heading has already said why every
        // tile is here; the tooltip keeps the other rules that also fired.
        let shown_reasons = if context.focus.stage_rule.is_some() {
            Vec::new()
        } else {
            reasons.clone()
        };
        let facts = judged
            .facts()
            .iter()
            .map(|fact| TileFact {
                label: fact.label().to_owned(),
                value: FactValueText::new(verdict.fact(fact.key()), fact.kind()),
            })
            .collect();
        let fact = match context.tested {
            Some((key, label)) => Some(TileFact {
                label: label.to_owned(),
                value: FactValueText::new(
                    verdict
                        .measured(key)
                        .map(|value| serde_json::json!(value))
                        .as_ref(),
                    FactKind::Number,
                ),
            }),
            None => context.focus.lens.as_ref().map(|lens| TileFact {
                label: lens.label().to_owned(),
                value: FactValueText::new(verdict.fact(lens.key()), lens.kind()),
            }),
        };
        let fill = verdict
            .episode()
            .duration_seconds()
            .filter(|_| context.longest > 0.0)
            .map_or(0.0, |seconds| seconds / context.longest);
        Self {
            reasons,
            shown_reasons,
            fact,
            facts,
            fill,
            highlight: TileHighlight::for_rule(context.focus.hovered_rule.as_deref(), verdict),
            selected: context.focus.open_episode.as_ref() == Some(verdict.episode().id()),
        }
    }
}

#[component]
fn EpisodeTile(
    tile: SheetTile,
    look: TileLook,
    position: usize,
    on_select: EventHandler<EpisodeId>,
) -> Element {
    let episode = tile.verdict.episode();
    let id = episode.id().clone();
    let duration = episode.duration_label();
    // Every measured fact and every reason travels in the tooltip; the tile
    // itself prints one so the whole dataset stays on one sheet.
    let mut detail = vec![
        episode
            .source_uri()
            .map_or_else(|| id.to_string(), str::to_owned),
    ];
    detail.extend(
        look.facts
            .iter()
            .map(|fact| format!("{}: {}", fact.label, fact.value.full())),
    );
    detail.extend(
        look.reasons
            .iter()
            .map(|reason| format!("Excluded: {}", reason.text)),
    );
    let title = detail.join("\n");
    let more_reasons = look.shown_reasons.len().saturating_sub(1);
    let tone = look.reasons.first().map_or_else(
        || "var(--tone-line-strong)".to_owned(),
        |reason| reason.tone.css(),
    );
    rsx! {
        li {
            class: "curation-tile",
            "data-kept": tile.verdict.kept(),
            "data-selected": look.selected,
            "data-highlight": look.highlight.attribute(),
            style: "--tile: {position.min(48)}; --tile-tone: {tone}; --fill: {look.fill * 100.0:.1}%",
            Button {
                purpose: ActionPurpose::Select,
                variant: ButtonVariant::Quiet,
                class: "curation-tile-face",
                id: SheetTile::element_id(episode.id()),
                aria_pressed: look.selected,
                title: "{title}",
                onclick: move |_| on_select.call(id.clone()),
                div { class: "curation-tile-head",
                    span { class: "curation-tile-id", "…{episode.id().short()}" }
                    span { class: "curation-tile-name", "{tile.label}" }
                }
                div { class: "curation-tile-line",
                    span { class: "curation-tile-clock", "{duration}" }
                    if let Some(fact) = look.fact.as_ref() {
                        span { class: "curation-tile-fact", title: "{fact.label}: {fact.value.full()}", "{fact.value}" }
                    }
                }
                // A struck tile prints the whole rule that removed it, wrapping
                // rather than clipping: the reason is the point of the tile.
                if let Some(reason) = look.shown_reasons.first() {
                    div { class: "curation-tile-reasons",
                        span { class: "curation-reason", style: "--hue: {reason.tone.css()}", "{reason.text}" }
                        if more_reasons > 0 {
                            span { class: "curation-reason-more", title: "{more_reasons} more rules fired", "+{more_reasons}" }
                        }
                    }
                }
            }
        }
    }
}

/// Eight resting tiles while Spiral reads the episodes.
#[component]
fn SheetSkeleton() -> Element {
    rsx! {
        ol { class: "curation-sheet-grid", "aria-hidden": "true",
            for index in 0..8 {
                li { class: "curation-tile is-skeleton", style: "--tile: {index}" }
            }
        }
    }
}

/// A fact value as the tile prints it: compact, exact in the tooltip.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FactValueText {
    value: Option<serde_json::Value>,
    kind: FactKind,
}

impl FactValueText {
    pub(super) fn new(value: Option<&serde_json::Value>, kind: FactKind) -> Self {
        Self {
            value: value.cloned(),
            kind,
        }
    }
    /// The stored value, unabridged.
    pub(super) fn full(&self) -> String {
        match &self.value {
            None | Some(serde_json::Value::Null) => "—".to_owned(),
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
        }
    }
    /// What the sheet orders by: numbers as numbers, text after them, and
    /// nothing at all for a missing value.
    fn sort_key(&self) -> Option<SortKey> {
        match &self.value {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Number(number)) => number.as_f64().map(SortKey::Number),
            Some(serde_json::Value::Bool(flag)) => {
                Some(SortKey::Number(f64::from(u8::from(*flag))))
            }
            Some(serde_json::Value::String(text)) => Some(SortKey::Text(text.to_lowercase())),
            Some(other) => Some(SortKey::Text(other.to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
enum SortKey {
    Number(f64),
    Text(String),
}

impl std::fmt::Display for FactValueText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.value {
            None | Some(serde_json::Value::Null) => formatter.write_str("—"),
            Some(serde_json::Value::Number(number)) => match number.as_f64() {
                Some(value) if self.kind == FactKind::Number && value.fract() != 0.0 => {
                    if value.abs() < 1.0 {
                        write!(formatter, "{value:.3}")
                    } else {
                        write!(formatter, "{value:.2}")
                    }
                }
                _ => write!(formatter, "{number}"),
            },
            Some(serde_json::Value::Bool(flag)) => {
                formatter.write_str(if *flag { "yes" } else { "no" })
            }
            Some(serde_json::Value::String(text)) => formatter.write_str(text),
            Some(other) => write!(formatter, "{other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::curation::facts::{CurationFact, FactMeasure};
    use spiral_robotics_common::curation::run::{CurationTotals, EpisodeFacts};

    /// A verdict for the fixture episode `ordinal`, recorded at `uri`.
    fn verdict(ordinal: u32, uri: &str) -> CurationVerdict {
        let episode: EpisodeFacts = serde_json::from_value(serde_json::json!({
            "id": EpisodeId::fixture(ordinal), "source_uri": uri,
        }))
        .unwrap();
        CurationVerdict::new(episode)
    }

    fn judged(ordinal: u32, uri: &str, brightness: f64) -> CurationVerdict {
        verdict(ordinal, uri).with_fact("brightness", serde_json::json!(brightness))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn fact_values_print_compactly_and_stay_exact_in_full() {
        let value = serde_json::json!(0.186834);
        let text = FactValueText::new(Some(&value), FactKind::Number);
        assert_eq!(text.to_string(), "0.187");
        assert_eq!(text.full(), "0.186834");
        assert_eq!(
            FactValueText::new(Some(&serde_json::json!(12)), FactKind::Number).to_string(),
            "12"
        );
        assert_eq!(FactValueText::new(None, FactKind::Text).to_string(), "—");
        assert_eq!(RuleTone::at(6), RuleTone::at(0));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_roster_keeps_each_tiles_id_while_ordering_by_the_lens() {
        let outcome = CurationOutcome::new(
            CurationTotals::new(3, 3, (3, false)),
            vec![
                judged(1, "hf://d/a/episode.mcap", 0.5),
                judged(2, "hf://d/b/episode.mcap", 0.1),
                verdict(3, "hf://d/c/episode.mcap"),
            ],
        );
        let lens = CurationFact::new(
            "brightness",
            "Camera brightness",
            FactMeasure::Sql {
                sql: "SELECT id, value".to_owned(),
                value: FactKind::Number,
            },
        );
        let ids = |order: SheetOrder| {
            Roster::new(Some(&outcome), SheetFilter::All, Some(&lens), order)
                .tiles
                .iter()
                .map(|tile| tile.verdict().episode().id().clone())
                .collect::<Vec<_>>()
        };
        let [first, second, third] = [1, 2, 3].map(EpisodeId::fixture);
        assert_eq!(
            ids(SheetOrder::Recorded),
            [first.clone(), second.clone(), third.clone()]
        );
        assert_eq!(
            ids(SheetOrder::Ascending),
            [second.clone(), first.clone(), third.clone()]
        );
        assert_eq!(
            ids(SheetOrder::Descending),
            [first.clone(), second.clone(), third]
        );
        let descending = Roster::new(
            Some(&outcome),
            SheetFilter::All,
            Some(&lens),
            SheetOrder::Descending,
        );
        assert_eq!(descending.position_of(&second), Some(1));
        assert_eq!(
            SheetTile::element_id(&first),
            "curation-tile-00000000000000000000000000000001"
        );
    }
}
