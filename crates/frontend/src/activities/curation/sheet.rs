//! The contact sheet: one tile per episode, kept tiles in full ink, excluded
//! tiles dimmed under the rule that removed them. Touch a rule's mark in the
//! recipe and the tiles it struck light up; open a tile and its evidence
//! docks beside the sheet.

use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationSpec;
use spiral_robotics_common::curation::facts::{CurationFact, FactKind};
use spiral_robotics_common::curation::run::{CurationOutcome, CurationVerdict};
use spiral_robotics_common::runs::RunPhase;

use super::episode::EpisodeSheet;
use super::state::{CurationController, SheetFilter, SheetOrder};
use crate::components::document_bar::ExecutedSql;
use crate::components::icons::{CurationIcon, ResultNodeIcon, SpinnerIcon};
use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, BrowserFocus, ModeAction, ModeGroup,
};
use crate::components::run_phase::RunPhasePresentation;
use crate::components::side_sheet::SheetPosition;
use crate::format::ClockPresentation;
use crate::shell::state::StudioProjectState;

/// Focus lands here when a tile opens its evidence.
pub(super) const EPISODE_HEADING_ID: &str = "curation-episode-heading";

/// The six identity washes, dealt to rules in recipe order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RuleTone(usize);

impl RuleTone {
    /// Dealt so the first rules sit far from role blue: a lit ring must never
    /// be mistaken for the selected tile's ring. Blue comes last.
    const DEAL: [usize; 6] = [2, 4, 3, 5, 6, 1];
    pub(super) const fn at(index: usize) -> Self {
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
    let outcome = controller.outcome();
    let judged = controller.judged_spec();
    let runs = controller.curation_runs();
    let filter = controller.filter();
    let order = controller.order();
    let hovered = controller.hovered_rule();
    let selected = controller.selected_episode();
    let phase = snapshot.phase();
    let lens = judged.as_ref().and_then(|judged| controller.lens(judged));
    let roster = Roster::new(outcome.as_ref(), filter, lens.as_ref(), order);
    // Opening a tile moves focus into its evidence; opening it again closes.
    let select = move |id: String| {
        if controller.selected_episode().as_deref() == Some(id.as_str()) {
            controller.select_episode(None);
        } else {
            controller.select_episode(Some(id));
            BrowserFocus::after_render(EPISODE_HEADING_ID.to_owned());
        }
    };
    let open = selected
        .as_deref()
        .and_then(|id| roster.position_of(id))
        .map(|index| SheetPosition::new(index, roster.len()));
    rsx! {
        section { class: "curation-sheet", "aria-label": "Curation sheet",
            if !runs.is_empty() {
                div { class: "run-strip curation-run-strip",
                    span { class: "run-phase {phase.class_name()}", role: "status",
                        span { class: "run-phase-dot", "aria-hidden": "true" }
                        "{phase.run_label()}"
                    }
                    if runs.len() > 1 {
                        label { r#for: "curation-result-run", class: "sr-only", "Run" }
                        select { id: "curation-result-run", class: "run-select",
                            value: snapshot.run_id().unwrap_or_default(),
                            onchange: move |event| controller.select_run(event.value()),
                            for (index, run) in runs.iter().enumerate() {
                                option { key: "{run.snapshot().run_id().unwrap_or_default()}",
                                    value: run.snapshot().run_id().unwrap_or_default(),
                                    selected: run.snapshot().run_id() == snapshot.run_id(),
                                    if run.snapshot().phase() == RunPhase::Completed { "Run {index + 1} of {runs.len()}" }
                                    else { "Run {index + 1} of {runs.len()} · {run.snapshot().phase().run_label().to_ascii_lowercase()}" }
                                }
                            }
                        }
                    }
                    if let (Some(outcome), Some(judged)) = (outcome.as_ref(), judged.as_ref()) {
                        // The filter carries the totals: each segment says how
                        // many tiles it shows, so no separate count line is needed.
                        ModeGroup { label: "Show".to_owned(), class: "curation-sheet-filter".to_owned(),
                            for (option, count) in [
                                (SheetFilter::All, outcome.totals().episodes()),
                                (SheetFilter::Kept, outcome.totals().kept()),
                                (SheetFilter::Excluded, outcome.totals().excluded()),
                            ] {
                                ModeAction { purpose: ActionPurpose::Select, checked: filter == option,
                                    onclick: move |_| controller.set_filter(option),
                                    "{option.label()} " strong { "{count}" } }
                            }
                        }
                        if !judged.facts().is_empty() {
                            SheetLens { controller, judged: judged.clone(), lens: lens.clone(), order }
                        }
                        span { class: "run-duration", title: "Spiral judging time", "{ElapsedText(outcome.elapsed_ms())}" }
                        if outcome.totals().truncated() {
                            span { class: "chip chip-caution", title: "The sheet lists the first {outcome.totals().listed()} episodes; the counts cover every episode.", "Sheet truncated" }
                        }
                    }
                    if phase.active() {
                        span { class: "run-elapsed", role: "status", "Reading episodes…" }
                    }
                    if let Some(sql) = snapshot.sql() {
                        ExecutedSql { sql: sql.to_owned() }
                    }
                }
            }
            if snapshot.result_stale() && outcome.is_some() {
                div { class: "results-stale", role: "status",
                    strong { "Previous run" }
                    span { "The recipe has changed since these verdicts. Run again." }
                }
            }
            div { class: "curation-sheet-stage",
                div { class: "curation-sheet-body",
                    if !controller.loaded() && controller.load_error().is_none() {
                        div { class: "panel-empty", role: "status", SpinnerIcon {} strong { "Loading curation…" } }
                    } else if let Some(error) = controller.load_error() {
                        div { class: "panel-empty", role: "alert",
                            strong { "Could not load the saved curation" }
                            p { "{error}" }
                            Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| controller.reload_current(), "Try again" }
                        }
                    } else if let Some(judged) = judged.as_ref().filter(|_| outcome.is_some()) {
                        TileGrid {
                            roster: roster.clone(), judged: judged.clone(), lens: lens.clone(),
                            filter, hovered, selected: selected.clone(), on_select: select,
                        }
                    } else if phase.active() {
                        SheetSkeleton {}
                    } else if phase == RunPhase::Failed {
                        div { class: "panel-empty", role: "alert",
                            strong { "Run failed" }
                            p { class: "curation-error-detail", "{snapshot.error().unwrap_or(\"Spiral did not say why.\")}" }
                            if controller.run_disabled_reason().is_none() {
                                Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline, onclick: move |_| controller.run(studio), "Run again" }
                            }
                        }
                    } else if spec.source().is_none() {
                        div { class: "panel-empty",
                            CurationIcon {}
                            strong { "Name a dataset" }
                            p { "Every one of its episodes becomes a tile here, kept or excluded, with the rule that decided." }
                        }
                    } else {
                        div { class: "panel-empty",
                            ResultNodeIcon {}
                            strong { "Nothing judged yet" }
                            p { "Run the recipe to lay the episodes out." }
                        }
                    }
                }
                if let (Some(position), Some(judged)) = (open, judged.as_ref()) {
                    if let Some(tile) = roster.get(position.index()) {
                        EpisodeSheet {
                            key: "{tile.verdict().episode().id()}",
                            tile: tile.clone(),
                            judged: judged.clone(),
                            position,
                            on_step: {
                                let roster = roster.clone();
                                move |delta: isize| {
                                    if let Some(next) = position.stepped(delta).and_then(|next| roster.get(next.index())) {
                                        controller.select_episode(Some(next.verdict().episode().id().to_owned()));
                                        BrowserFocus::after_render(EPISODE_HEADING_ID.to_owned());
                                    }
                                }
                            },
                            on_close: {
                                let ordinal = tile.ordinal();
                                move |_| {
                                    controller.select_episode(None);
                                    BrowserFocus::after_render(SheetTile::element_id(ordinal));
                                }
                            },
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
            select { id: "curation-lens", class: "run-select", title: "The measurement every tile prints",
                value: lens.as_ref().map(|fact| fact.key().to_owned()).unwrap_or_default(),
                onchange: move |event| controller.set_lens(event.value()),
                for fact in judged.facts() {
                    option { value: "{fact.key()}", selected: lens.as_ref().is_some_and(|lens| lens.key() == fact.key()), "{fact.label()}" }
                }
            }
            Action { purpose: ActionPurpose::Toggle, variant: ActionVariant::Quiet, class: "curation-order",
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
        let labels = SheetLabels::new(outcome.verdicts());
        let mut tiles = outcome
            .verdicts()
            .iter()
            .enumerate()
            .filter(|(_, verdict)| filter.admits(verdict.kept()))
            .map(|(index, verdict)| SheetTile {
                ordinal: index + 1,
                label: labels.label(verdict),
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
    fn len(&self) -> usize {
        self.tiles.len()
    }
    fn get(&self, index: usize) -> Option<&SheetTile> {
        self.tiles.get(index)
    }
    fn position_of(&self, episode_id: &str) -> Option<usize> {
        self.tiles
            .iter()
            .position(|tile| tile.verdict.episode().id() == episode_id)
    }
}

/// One episode as the sheet knows it: its place in the dataset, the words
/// that tell it from its neighbours, and its verdict.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SheetTile {
    ordinal: usize,
    label: String,
    verdict: CurationVerdict,
}

impl SheetTile {
    pub(super) const fn ordinal(&self) -> usize {
        self.ordinal
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    pub(super) const fn verdict(&self) -> &CurationVerdict {
        &self.verdict
    }
    /// The tile's own button, where focus returns when its evidence closes.
    pub(super) fn element_id(ordinal: usize) -> String {
        format!("curation-tile-{ordinal}")
    }
}

#[component]
fn TileGrid(
    roster: Roster,
    judged: CurationSpec,
    lens: Option<CurationFact>,
    filter: SheetFilter,
    hovered: Option<String>,
    selected: Option<String>,
    on_select: EventHandler<String>,
) -> Element {
    if roster.len() == 0 {
        return rsx! {
            div { class: "panel-empty is-quiet",
                p { match filter {
                    SheetFilter::Kept => "No episode was kept.",
                    SheetFilter::Excluded => "No episode was excluded.",
                    SheetFilter::All => "The dataset has no episodes.",
                } }
            }
        };
    }
    let catalog = judged.catalog();
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
                    look: TileLook::of(tile, &judged, &catalog, TileContext { lens: lens.as_ref(), hovered: hovered.as_deref(), selected: selected.as_deref(), longest }),
                    position,
                    on_select,
                }
            }
        }
    }
}

/// What every tile is judged against when it is drawn.
#[derive(Clone, Copy)]
struct TileContext<'a> {
    lens: Option<&'a CurationFact>,
    hovered: Option<&'a str>,
    selected: Option<&'a str>,
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
    fact: Option<TileFact>,
    facts: Vec<TileFact>,
    fill: f64,
    highlight: TileHighlight,
    selected: bool,
}

impl TileLook {
    fn of(
        tile: &SheetTile,
        judged: &CurationSpec,
        catalog: &spiral_robotics_common::curation::facts::FactCatalog,
        context: TileContext<'_>,
    ) -> Self {
        let verdict = &tile.verdict;
        let reasons = verdict
            .excluded_by()
            .iter()
            .map(|rule_id| Reason {
                text: judged
                    .rules()
                    .iter()
                    .find(|rule| rule.id() == rule_id)
                    .map_or_else(|| rule_id.to_owned(), |rule| rule.reason(catalog)),
                tone: RuleTone::of(judged, rule_id),
            })
            .collect();
        let facts = judged
            .facts()
            .iter()
            .map(|fact| TileFact {
                label: fact.label().to_owned(),
                value: FactValueText::new(verdict.fact(fact.key()), fact.kind()),
            })
            .collect();
        let fact = context.lens.map(|lens| TileFact {
            label: lens.label().to_owned(),
            value: FactValueText::new(verdict.fact(lens.key()), lens.kind()),
        });
        let fill = verdict
            .episode()
            .duration_seconds()
            .filter(|_| context.longest > 0.0)
            .map_or(0.0, |seconds| seconds / context.longest);
        Self {
            reasons,
            fact,
            facts,
            fill,
            highlight: TileHighlight::for_rule(context.hovered, verdict),
            selected: context.selected == Some(verdict.episode().id()),
        }
    }
}

#[component]
fn EpisodeTile(
    tile: SheetTile,
    look: TileLook,
    position: usize,
    on_select: EventHandler<String>,
) -> Element {
    let episode = tile.verdict.episode();
    let id = episode.id().to_owned();
    let duration = episode
        .duration_seconds()
        .map_or_else(|| "—".to_owned(), ClockPresentation::clock_label);
    // Every measured fact and every reason travels in the tooltip; the tile
    // itself prints one so the whole dataset stays on one sheet.
    let mut detail = vec![
        episode
            .source_uri()
            .map_or_else(|| id.clone(), str::to_owned),
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
    let more_reasons = look.reasons.len().saturating_sub(1);
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
            Action {
                purpose: ActionPurpose::Select,
                variant: ActionVariant::Quiet,
                class: "curation-tile-face",
                id: SheetTile::element_id(tile.ordinal),
                aria_pressed: look.selected,
                title: "{title}",
                onclick: move |_| on_select.call(id.clone()),
                div { class: "curation-tile-head",
                    span { class: "curation-tile-ordinal", "{tile.ordinal}" }
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
                if let Some(reason) = look.reasons.first() {
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

/// The part of each source path that differs between episodes.
struct SheetLabels {
    shared_prefix: usize,
}

impl SheetLabels {
    fn new(verdicts: &[CurationVerdict]) -> Self {
        let mut uris = verdicts
            .iter()
            .filter_map(|verdict| verdict.episode().source_uri());
        let Some(first) = uris.next() else {
            return Self { shared_prefix: 0 };
        };
        let mut shared = first.len();
        for uri in uris {
            shared = shared.min(
                first
                    .bytes()
                    .zip(uri.bytes())
                    .take_while(|(left, right)| left == right)
                    .count(),
            );
        }
        // Cut back to a directory boundary so a label never starts mid-word.
        let shared_prefix = first[..shared].rfind('/').map_or(0, |slash| slash + 1);
        Self { shared_prefix }
    }
    /// The first distinguishing path segment, else the file name.
    fn label(&self, verdict: &CurationVerdict) -> String {
        let episode = verdict.episode();
        let relative = episode
            .source_uri()
            .and_then(|uri| uri.get(self.shared_prefix..))
            .filter(|relative| !relative.is_empty());
        match relative {
            Some(relative) => relative
                .split('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or(relative)
                .replace('_', " "),
            None => episode.file_name().to_owned(),
        }
    }
}

/// Elapsed time in the unit a person would say.
struct ElapsedText(u64);

impl std::fmt::Display for ElapsedText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let millis = self.0;
        if millis < 1_000 {
            write!(formatter, "{millis} ms")
        } else if millis < 60_000 {
            write!(formatter, "{:.1} s", millis as f64 / 1000.0)
        } else {
            write!(
                formatter,
                "{}m {:02}s",
                millis / 60_000,
                (millis / 1000) % 60
            )
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

    fn verdict(uri: &str) -> CurationVerdict {
        let episode: EpisodeFacts =
            serde_json::from_value(serde_json::json!({"id": uri, "source_uri": uri})).unwrap();
        CurationVerdict::new(episode)
    }

    fn judged(uri: &str, brightness: f64) -> CurationVerdict {
        verdict(uri).with_fact("brightness", serde_json::json!(brightness))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn tile_labels_are_the_distinguishing_directory_with_words_not_underscores() {
        let verdicts = vec![
            verdict("hf://d/data/val/fold_and_stack_the_skirts/episode_a/episode.fo.mcap"),
            verdict("hf://d/data/val/place_the_fruits/episode_b/episode.fo.mcap"),
        ];
        let labels = SheetLabels::new(&verdicts);
        assert_eq!(labels.label(&verdicts[0]), "fold and stack the skirts");
        assert_eq!(labels.label(&verdicts[1]), "place the fruits");
        let single = vec![verdict("file:///runs/take-7.mcap")];
        assert_eq!(SheetLabels::new(&single).label(&single[0]), "take-7.mcap");
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
        assert_eq!(ElapsedText(640).to_string(), "640 ms");
        assert_eq!(ElapsedText(12_400).to_string(), "12.4 s");
        assert_eq!(ElapsedText(125_000).to_string(), "2m 05s");
        assert_eq!(RuleTone::at(6), RuleTone::at(0));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_roster_keeps_dataset_ordinals_while_ordering_by_the_lens() {
        let outcome = CurationOutcome::new(
            CurationTotals::new(3, 3, (3, false)),
            vec![
                judged("hf://d/a/episode.mcap", 0.5),
                judged("hf://d/b/episode.mcap", 0.1),
                verdict("hf://d/c/episode.mcap"),
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
        let recorded = Roster::new(
            Some(&outcome),
            SheetFilter::All,
            Some(&lens),
            SheetOrder::Recorded,
        );
        assert_eq!(
            recorded
                .tiles
                .iter()
                .map(SheetTile::ordinal)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let ascending = Roster::new(
            Some(&outcome),
            SheetFilter::All,
            Some(&lens),
            SheetOrder::Ascending,
        );
        assert_eq!(
            ascending
                .tiles
                .iter()
                .map(SheetTile::ordinal)
                .collect::<Vec<_>>(),
            [2, 1, 3]
        );
        let descending = Roster::new(
            Some(&outcome),
            SheetFilter::All,
            Some(&lens),
            SheetOrder::Descending,
        );
        assert_eq!(
            descending
                .tiles
                .iter()
                .map(SheetTile::ordinal)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(descending.position_of("hf://d/b/episode.mcap"), Some(1));
        assert_eq!(SheetTile::element_id(7), "curation-tile-7");
    }
}
