//! A node's rule in the words of what it keeps by: a Select's conditions on
//! the episodes' recorded facts, one row each beside the strip its value
//! slides on, and Score progress's progress rate.
use super::{RuleKnob, RuleStrip, RuleView};
use crate::activities::curation::document::{Comparison, EpisodeCondition, EpisodeFact, Selection};
use crate::activities::curation::threshold::{Threshold, ThresholdStrip};
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, CloseIcon, Select};

/// A Select's conditions, one row each: the fact it reads and which side of
/// its value it keeps, the value, the strip that value slides on over every
/// episode reaching the Select, and how many of them it leaves out; then a
/// way to add another. An episode is kept when it meets every condition, so
/// each strip rings the episodes it passes that another condition leaves
/// out.
#[component]
pub(super) fn EpisodeConditions(
    view: RuleView,
    strips: Vec<RuleStrip>,
    chosen: Option<usize>,
    /// A dot chosen: the item it stands for, to open.
    on_select: EventHandler<usize>,
) -> Element {
    let Selection::Episodes { conditions } = view.selection.clone() else {
        return rsx! {};
    };
    let removable = conditions.len() > 1;
    // A new condition asks about a fact the rule does not read yet, from
    // where it keeps every episode.
    let added = {
        let mut conditions = conditions.clone();
        let fact = EpisodeFact::ALL
            .into_iter()
            .find(|fact| conditions.iter().all(|condition| condition.fact() != *fact))
            .unwrap_or(EpisodeFact::DurationSeconds);
        conditions.push(EpisodeCondition::new(fact, Comparison::AtLeast, 0.0));
        Selection::Episodes { conditions }
    };
    let rows = conditions
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, condition)| {
            let strip = strips
                .iter()
                .find(|strip| strip.knob == RuleKnob::Condition(index))?
                .clone();
            Some(ConditionRow {
                index,
                condition,
                strip,
                leaves_out: view.leaves_out(&condition),
            })
        })
        .collect::<Vec<_>>();
    let adding = view.clone();
    rsx! {
        div { class: "curation-select-conditions", role: "group", "aria-label": "Conditions every kept episode meets",
            for row in rows {
                ConditionLine {
                    key: "{row.index}",
                    view: view.clone(),
                    row,
                    removable,
                    pick: StripPick { chosen, on_select },
                }
            }
            div { class: "curation-select-condition-add",
                Button {
                    purpose: ActionPurpose::Create,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    title: "Keep only episodes that also meet another condition",
                    onclick: move |_| adding.set(added.clone()),
                    "Add condition"
                }
            }
        }
    }
}

/// One condition of a Select, with the strip its value slides on and how
/// many episodes it leaves out whatever the others say.
#[derive(Clone, Debug, PartialEq)]
struct ConditionRow {
    index: usize,
    condition: EpisodeCondition,
    strip: RuleStrip,
    leaves_out: usize,
}

/// Which dot of a strip is the item playing, and what choosing one does.
#[derive(Clone, PartialEq)]
struct StripPick {
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
}

/// "[duration] [at least] 20 s", its strip, "leaves out 4", and the way to
/// take it away while another remains.
#[component]
fn ConditionLine(view: RuleView, row: ConditionRow, removable: bool, pick: StripPick) -> Element {
    let ConditionRow {
        index,
        condition,
        strip,
        leaves_out,
    } = row;
    let fact = condition.fact();
    let with = {
        let view = view.clone();
        move |condition: EpisodeCondition| {
            if let Selection::Episodes { mut conditions } = view.selection.clone() {
                conditions[index] = condition;
                view.set(Selection::Episodes { conditions });
            }
        }
    };
    let threshold = strip.threshold();
    let unit = fact
        .unit()
        .map(|unit| format!(" {unit}"))
        .unwrap_or_default();
    rsx! {
        div { class: "curation-select-condition",
            Select {
                class: "curation-select-condition-fact",
                value: fact.key().to_owned(),
                "aria-label": "Recorded fact",
                onchange: {
                    let with = with.clone();
                    move |key: String| {
                        if let Some(fact) = EpisodeFact::from_key(&key) {
                            with(condition.on(fact));
                        }
                    }
                },
                for option in EpisodeFact::ALL {
                    option { key: "{option.key()}", value: "{option.key()}", selected: option == fact, "{option.name()}" }
                }
            }
            Select {
                class: "curation-select-condition-comparison",
                value: condition.comparison().key().to_owned(),
                "aria-label": "Comparison",
                onchange: {
                    let with = with.clone();
                    move |key: String| {
                        if let Some(comparison) = Comparison::from_key(&key) {
                            with(condition.comparing(comparison));
                        }
                    }
                },
                for option in Comparison::ALL {
                    option { key: "{option.key()}", value: "{option.key()}", selected: option == condition.comparison(), "{option.label()}" }
                }
            }
            output { class: "curation-select-condition-value", "{threshold.label()}{unit}" }
            ThresholdStrip {
                dots: strip.dots.clone(),
                threshold,
                chosen: pick.chosen,
                label: format!("{} threshold", fact.name()),
                on_select: move |item| pick.on_select.call(item),
                on_threshold: {
                    let with = with.clone();
                    move |value| with(condition.at(value))
                },
            }
            p { class: "curation-select-condition-count",
                if leaves_out == 0 { "leaves none out" } else { "leaves out {leaves_out}" }
            }
            if removable {
                Button {
                    purpose: ActionPurpose::Remove,
                    variant: ButtonVariant::Quiet,
                    size: ButtonSize::Compact,
                    class: "curation-select-condition-remove",
                    title: "Remove this condition",
                    "aria-label": "Remove the {fact.label()} condition",
                    onclick: move |_| {
                        if let Selection::Episodes { mut conditions } = view.selection.clone() {
                            conditions.remove(index);
                            view.set(Selection::Episodes { conditions });
                        }
                    },
                    CloseIcon {}
                }
            }
        }
    }
}

/// "Keep moments progressing at least 0.89× the expert rate".
#[component]
pub(in crate::activities::curation) fn ProgressRule(threshold: Option<Threshold>) -> Element {
    rsx! {
        div { class: "curation-select-rule",
            span { "Keep moments progressing at least" }
            if let Some(threshold) = threshold {
                output { class: "curation-select-rule-readout", "{threshold.label()}" }
            }
            span { "the expert rate" }
        }
    }
}
