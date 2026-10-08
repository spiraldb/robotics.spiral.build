//! A Select's conditions, one row each in the words of what it keeps by,
//! beside the strip its threshold slides on over every value it judges, and
//! how much it leaves out whatever the others say; then a way to add another
//! from what the nodes above offer.
//!
//! An episode is kept when it meets every condition, so each strip rings the
//! values its condition passes that another condition leaves out.
use super::SelectView;
use crate::activities::curation::document::{
    Comparison, CurationNode, CurationNodeKind, EpisodeCondition, SelectCondition,
};
use crate::activities::curation::episode_tabs::OpenEpisodes;
use crate::activities::curation::review::Counted;
use crate::activities::curation::threshold::{
    AxisUnit, DotSide, StripDot, Threshold, ThresholdStrip,
};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, CloseIcon, Select, Toggle,
};

/// Where one value of a strip plays: its episode, and its stretch unless the
/// value is the whole episode's.
type Place = (EpisodeId, Option<(f64, f64)>);

/// One condition's values on its strip: a dot for every value it judges,
/// where each plays, and its threshold.
#[derive(Clone, PartialEq)]
struct ConditionStrip {
    dots: Vec<StripDot>,
    places: Vec<Place>,
    threshold: Threshold,
}

impl ConditionStrip {
    /// The strip of the condition at `index`. An episode condition judges
    /// the reaching episodes, by their number or, ranked, their percent
    /// rank; a range condition every moment, picture, or subtask of them.
    fn of(view: &SelectView, index: usize, condition: &SelectCondition) -> Self {
        let episodes = view.reading.input().episodes();
        let mut values = Vec::new();
        let mut sides = Vec::new();
        let mut places = Vec::new();
        // One value: where it plays, whether this condition passes it, and
        // whether the Select keeps it in the end.
        let mut judge = |place: Place, value: Option<f64>, (passes, kept): (bool, bool)| {
            sides.push(match (passes, kept) {
                (false, _) => DotSide::LeftOut,
                (true, false) => DotSide::LeftOutElsewhere,
                (true, true) => DotSide::Kept,
            });
            values.push(value);
            places.push(place);
        };
        // Whether the Select keeps the middle of a stretch of `episode`.
        let kept_at = |episode: &EpisodeId, (start, end): (f64, f64)| {
            view.kept(episode).holds(f64::midpoint(start, end))
        };
        let (value, unit, keeps_above) = match condition {
            SelectCondition::Episodes(condition) => {
                let reading = view.reading.condition(index);
                let numbers = match reading {
                    Some(super::reading::ConditionReading::Episodes { numbers, .. }) => {
                        numbers.clone()
                    }
                    _ => vec![None; episodes.len()],
                };
                let shown = if condition.comparison().ranks() {
                    condition.ranks(&numbers)
                } else {
                    numbers
                };
                for (place, episode) in episodes.iter().enumerate() {
                    let passes = reading.is_some_and(|reading| reading.holds(place));
                    let kept = view.reading.output().of(episode).is_some();
                    judge((episode.clone(), None), shown[place], (passes, kept));
                }
                let unit = match (condition.comparison().ranks(), condition.number().counts()) {
                    (true, _) => AxisUnit::Percent,
                    (false, true) => AxisUnit::Count,
                    (false, false) => AxisUnit::Plain,
                };
                (
                    condition.value(),
                    unit,
                    condition.comparison() == Comparison::AtLeast,
                )
            }
            range => {
                view.controller.with_reading(|_, reader| {
                    let Some(reader) = reader else {
                        return;
                    };
                    for episode in &episodes {
                        for item in range.judged_items(episode, reader) {
                            let kept = kept_at(episode, item.span());
                            judge(
                                (episode.clone(), Some(item.span())),
                                item.value(),
                                (item.passes(), kept),
                            );
                        }
                    }
                });
                match range {
                    SelectCondition::Progressing { minimum, .. } => {
                        (*minimum, AxisUnit::ProgressRate, true)
                    }
                    SelectCondition::Visible { fewest, .. } => {
                        (*fewest as f64, AxisUnit::Count, true)
                    }
                    SelectCondition::Similar { minimum, .. } => (*minimum, AxisUnit::Percent, true),
                    _ => (0.0, AxisUnit::Plain, true),
                }
            }
        };
        let dots = StripDot::on_sides(&values, &sides);
        Self {
            threshold: Threshold::over(&dots, value, unit, keeps_above),
            dots,
            places,
        }
    }
}

/// One condition's row, as every kind of condition draws it: which
/// condition it is, its strip, how much it leaves out, which dot is playing
/// and what choosing one does, and whether it may be removed.
#[derive(Clone, PartialEq)]
struct ConditionRow {
    index: usize,
    strip: ConditionStrip,
    leaves: String,
    pick: StripPick,
    removable: bool,
}

/// Which dot of a strip is the item playing, and what choosing one does.
#[derive(Clone, PartialEq)]
struct StripPick {
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
}

impl ConditionRow {
    fn of(
        view: &SelectView,
        (index, condition): (usize, &SelectCondition),
        tabs: OpenEpisodes,
        removable: bool,
    ) -> Self {
        let strip = ConditionStrip::of(view, index, condition);
        let leaves = match view.reading.seconds_kept_by(index) {
            Some(kept) => format!(
                "keeps {} of {}",
                kept.clock_label(),
                view.reading.input().seconds().clock_label()
            ),
            None => match view.reading.leaves_out(index) {
                0 => "leaves none out".to_owned(),
                left => format!("leaves out {left}"),
            },
        };
        let places = strip.places.clone();
        let chosen = tabs.front().and_then(|tab| {
            places.iter().position(|(episode, span)| match span {
                None => tab.interval().is_none() && tab.episode() == episode,
                Some((start, _)) => tab.holds(episode, *start),
            })
        });
        let on_select = EventHandler::new(move |item: usize| {
            if let Some((episode, span)) = places.get(item) {
                tabs.open(episode, *span);
            }
        });
        Self {
            index,
            strip,
            leaves,
            pick: StripPick { chosen, on_select },
            removable,
        }
    }
}

impl SelectView {
    /// Put `condition` in the place of the one at `index`.
    fn replace(&self, index: usize, condition: SelectCondition) {
        self.set(self.selection.replacing(index, condition));
    }
}

/// Every condition of the Select, and the way to add another.
#[component]
pub(super) fn SelectConditions(view: SelectView, tabs: OpenEpisodes) -> Element {
    let conditions = view.selection.conditions().to_vec();
    let removable = conditions.len() > 1;
    let choices = view.with_graph(|graph| graph.conditions_for(&view.node));
    let labels = choices
        .iter()
        .map(|choice| choice.label().to_owned())
        .collect::<Vec<_>>();
    let adding = view.clone();
    rsx! {
        div { class: "curation-select-conditions", role: "group", "aria-label": "Conditions every kept episode meets",
            for (index, condition) in conditions.into_iter().enumerate() {
                ConditionLine {
                    key: "{index}",
                    view: view.clone(),
                    row: ConditionRow::of(&view, (index, &condition), tabs, removable),
                    condition,
                }
            }
            // A row of its own under the last condition, its menu on the
            // first column's edge: what the nodes above offer to keep by.
            div { class: "curation-select-condition-add",
                Select {
                    value: String::new(),
                    "aria-label": "Add a condition",
                    title: "Keep only what also meets another condition",
                    onchange: move |choice: String| {
                        if let Some(choice) = choice.parse::<usize>().ok().and_then(|choice| choices.get(choice)) {
                            adding.set(adding.selection.adding(choice.condition().clone()));
                        }
                    },
                    option { value: "", disabled: true, selected: true, "Add condition" }
                    for (place, label) in labels.into_iter().enumerate() {
                        option { key: "{place}", value: "{place}", "{label}" }
                    }
                }
            }
        }
    }
}

/// One condition, drawn as its kind is.
#[component]
fn ConditionLine(view: SelectView, condition: SelectCondition, row: ConditionRow) -> Element {
    match condition {
        SelectCondition::Episodes(condition) => rsx! {
            EpisodeConditionLine { view, condition, row }
        },
        SelectCondition::InCategories { .. } => rsx! {
            CategoryConditionLine { view, condition, row }
        },
        range => rsx! {
            RangeConditionLine { view, condition: range, row }
        },
    }
}

/// The way to take a condition away while another remains.
#[component]
fn RemoveCondition(view: SelectView, index: usize) -> Element {
    rsx! {
        Button {
            purpose: ActionPurpose::Remove,
            variant: ButtonVariant::Quiet,
            size: ButtonSize::Compact,
            class: "curation-select-condition-remove",
            title: "Remove this condition",
            "aria-label": "Remove this condition",
            onclick: move |_| view.set(view.selection.without(index)),
            CloseIcon {}
        }
    }
}

/// "[Progress rate] at least 0.20×", its strip over every moment, picture
/// or subtask it judges, and how much of what reaches the Select it keeps.
#[component]
fn RangeConditionLine(view: SelectView, condition: SelectCondition, row: ConditionRow) -> Element {
    let label = view.with_graph(|graph| condition.name(graph));
    let threshold = row.strip.threshold;
    let (index, pick) = (row.index, row.pick.clone());
    let sliding = view.clone();
    rsx! {
        div { class: "curation-select-condition",
            span { class: "curation-select-condition-fact", "{label}" }
            span { class: "curation-select-condition-comparison", "at least" }
            output { class: "curation-select-condition-value", "{threshold.label()}" }
            ThresholdStrip {
                dots: row.strip.dots.clone(),
                threshold,
                chosen: pick.chosen,
                label: format!("{label} threshold"),
                on_select: move |item| pick.on_select.call(item),
                on_threshold: move |threshold: Threshold| {
                    sliding.replace(index, condition.at(threshold.value()));
                },
            }
            p { class: "curation-select-condition-count", "{row.leaves}" }
            if row.removable {
                RemoveCondition { view, index }
            }
        }
    }
}

/// "[duration per bottle] [lowest] 25%", its strip, and how many episodes it
/// leaves out.
#[component]
fn EpisodeConditionLine(
    view: SelectView,
    condition: EpisodeCondition,
    row: ConditionRow,
) -> Element {
    let numbers = view.with_graph(|graph| {
        graph
            .numbers_for(&view.node)
            .into_iter()
            .map(|number| {
                let name = number.name(graph);
                (number, name)
            })
            .collect::<Vec<_>>()
    });
    let current = numbers
        .iter()
        .position(|(number, _)| number == condition.number());
    let threshold = row.strip.threshold;
    let readout = if condition.comparison().ranks() {
        threshold.label()
    } else {
        let unit = condition
            .number()
            .unit()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        format!("{}{unit}", threshold.label())
    };
    let leaves_any = row.leaves != "leaves none out";
    let (index, pick) = (row.index, row.pick.clone());
    let comparison = condition.comparison();
    let with = {
        let view = view.clone();
        move |condition: EpisodeCondition| view.replace(index, SelectCondition::Episodes(condition))
    };
    let (choosing, comparing, sliding) = (numbers.clone(), condition.clone(), condition.clone());
    let (choose, compare, slide) = (with.clone(), with.clone(), with);
    rsx! {
        div { class: "curation-select-condition",
            Select {
                class: "curation-select-condition-fact",
                value: current.map(|place| place.to_string()).unwrap_or_default(),
                "aria-label": "Number",
                onchange: move |place: String| {
                    if let Some((number, _)) = place.parse::<usize>().ok().and_then(|place| choosing.get(place)) {
                        choose(condition.on(number.clone()));
                    }
                },
                for (place, (_, name)) in numbers.iter().enumerate() {
                    option { key: "{place}", value: "{place}", selected: Some(place) == current, "{name}" }
                }
            }
            Select {
                class: "curation-select-condition-comparison",
                value: comparison.key().to_owned(),
                "aria-label": "Comparison",
                onchange: move |key: String| {
                    let Some(comparison) = Comparison::from_key(&key) else {
                        return;
                    };
                    // A condition that leaves nothing out still leaves
                    // nothing out once turned around, from the other end of
                    // its strip, rather than suddenly leaving out every
                    // episode.
                    let flipped = comparing.comparing(comparison);
                    let (low, high) = threshold.ends();
                    compare(match comparison {
                        _ if leaves_any || comparison.ranks() => flipped,
                        Comparison::AtLeast => flipped.at(low),
                        _ => flipped.at(high),
                    });
                },
                for option in Comparison::ALL {
                    option { key: "{option.key()}", value: "{option.key()}", selected: option == comparison, "{option.label()}" }
                }
            }
            output { class: "curation-select-condition-value", "{readout}" }
            ThresholdStrip {
                dots: row.strip.dots.clone(),
                threshold,
                chosen: pick.chosen,
                label: "Threshold".to_owned(),
                on_select: move |item| pick.on_select.call(item),
                on_threshold: move |threshold: Threshold| slide(sliding.at(threshold.value())),
            }
            p { class: "curation-select-condition-count", "{row.leaves}" }
            if row.removable {
                RemoveCondition { view, index }
            }
        }
    }
}

/// "In [fold] [grasp] …": the categories of a grouping above, checked when
/// the Select keeps their subtasks.
#[component]
fn CategoryConditionLine(
    view: SelectView,
    condition: SelectCondition,
    row: ConditionRow,
) -> Element {
    let SelectCondition::InCategories { node, categories } = condition else {
        return rsx! {};
    };
    let offered = view.with_graph(|graph| match graph.node(&node).map(CurationNode::kind) {
        Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => grouping.categories().to_vec(),
        _ => Vec::new(),
    });
    let counts = view.controller.with_reading(|_, reader| {
        let rows = reader
            .map(|reader| {
                view.reading
                    .input()
                    .episodes()
                    .iter()
                    .flat_map(|episode| reader.subtasks(&node, episode).to_vec())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        offered
            .iter()
            .map(|category| {
                rows.iter()
                    .filter(|row| row.category() == Some(category.name()))
                    .count()
            })
            .collect::<Vec<_>>()
    });
    let index = row.index;
    rsx! {
        div { class: "curation-select-condition curation-select-condition-categories",
            span { class: "curation-select-condition-fact", "In categories" }
            div { class: "curation-select-categories", role: "group", "aria-label": "Categories to keep",
                if offered.is_empty() {
                    span { class: "curation-select-condition-count", "The grouping has no categories yet" }
                }
                for (place, category) in offered.iter().enumerate() {
                    Toggle {
                        key: "{category.id()}",
                        label: format!("{} · {}", category.name(), Counted::subtasks(counts[place])),
                        checked: categories.contains(&category.id()),
                        title: category.definition().to_owned(),
                        onchange: {
                            let (view, node, chosen, id) = (view.clone(), node.clone(), categories.clone(), category.id());
                            move |keep: bool| {
                                let mut chosen = chosen.clone();
                                chosen.retain(|other| *other != id);
                                if keep {
                                    chosen.push(id);
                                }
                                view.replace(index, SelectCondition::InCategories { node: node.clone(), categories: chosen });
                            }
                        },
                    }
                }
            }
            if row.removable {
                RemoveCondition { view, index }
            }
        }
    }
}

impl SelectCondition {
    /// The same range condition at `value`, on its own scale: a count of
    /// objects is whole.
    fn at(&self, value: f64) -> Self {
        match self {
            Self::Progressing { node, .. } => Self::Progressing {
                node: node.clone(),
                minimum: value,
            },
            Self::Visible { node, .. } => Self::Visible {
                node: node.clone(),
                fewest: value.round().max(0.0) as u64,
            },
            Self::Similar { node, .. } => Self::Similar {
                node: node.clone(),
                minimum: value.clamp(0.0, 1.0),
            },
            other => other.clone(),
        }
    }
}
