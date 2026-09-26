//! A node's rule, written as one sentence in the words of what it keeps by,
//! with the sentence's variable words as its fields: a Select's episode
//! conditions and categories, and Score progress's progress rate.
use super::RuleView;
use crate::activities::curation::document::{
    Comparison, CurationNodeKind, EpisodeCondition, EpisodeFact, Selection,
};
use crate::activities::curation::population::Population;
use crate::activities::curation::similar::CategoryHues;
use crate::activities::curation::state::CurationController;
use crate::activities::curation::threshold::Threshold;
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, CloseIcon, Select};

/// One node's rule, edited through the controller.
#[derive(Clone, PartialEq)]
pub(super) struct RuleEdit {
    node: String,
    controller: CurationController,
}

impl RuleEdit {
    pub(super) fn new(node: String, controller: CurationController) -> Self {
        Self { node, controller }
    }
    /// Replace the node's rule. The document refuses a rule for another
    /// basis, so a slip can never re-point a node at something else.
    pub(super) fn set(&self, rule: Selection) {
        let _ = self.controller.set_rule(&self.node, rule);
    }
}

/// The rule as one sentence. Its thresholds read out where the sentence says
/// them, in the order the strips under the bar slide them.
#[component]
pub(in crate::activities::curation) fn RuleSentence(
    controller: CurationController,
    view: RuleView,
    thresholds: Vec<Threshold>,
) -> Element {
    let edit = RuleEdit::new(view.node.clone(), controller);
    rsx! {
        div { class: "curation-select-rule",
            match view.selection.clone() {
                Selection::Episodes { conditions } => rsx! {
                    EpisodeRule { edit, conditions, thresholds }
                },
                Selection::Progressing { .. } => rsx! {
                    ProgressRule { threshold: thresholds.first().copied() }
                },
                Selection::Categories { names } => {
                    let categories = CategoryChoice::of(&view);
                    rsx! { CategoryRule { edit, names, categories } }
                }
                // A search's closeness is Find similar's own, on its ranking.
                Selection::Similar { .. } => rsx! {},
            }
        }
    }
}

/// "Keep episodes whose [duration] is [at least] 20 s and whose …", with a
/// way to add a condition and to take any but the last one away.
#[component]
fn EpisodeRule(
    edit: RuleEdit,
    conditions: Vec<EpisodeCondition>,
    thresholds: Vec<Threshold>,
) -> Element {
    let with = {
        let conditions = conditions.clone();
        move |index: usize, condition: EpisodeCondition| {
            let mut conditions = conditions.clone();
            conditions[index] = condition;
            Selection::Episodes { conditions }
        }
    };
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
    let removable = conditions.len() > 1;
    rsx! {
        span { "Keep episodes" }
        for (index, condition) in conditions.iter().copied().enumerate() {
            span { key: "{index}", class: "curation-select-condition",
                span { if index == 0 { "whose" } else { "and whose" } }
                Select {
                    class: "curation-select-rule-choice",
                    value: condition.fact().key().to_owned(),
                    "aria-label": "Recorded fact",
                    onchange: {
                        let edit = edit.clone();
                        let with = with.clone();
                        move |key: String| {
                            if let Some(fact) = EpisodeFact::from_key(&key) {
                                edit.set(with(index, condition.on(fact)));
                            }
                        }
                    },
                    for option in EpisodeFact::ALL {
                        option { key: "{option.key()}", value: "{option.key()}", selected: option == condition.fact(), "{option.label()}" }
                    }
                }
                span { "is" }
                Select {
                    class: "curation-select-rule-choice",
                    value: condition.comparison().key().to_owned(),
                    "aria-label": "Comparison",
                    onchange: {
                        let edit = edit.clone();
                        let with = with.clone();
                        move |key: String| {
                            if let Some(comparison) = Comparison::from_key(&key) {
                                edit.set(with(index, condition.comparing(comparison)));
                            }
                        }
                    },
                    for option in Comparison::ALL {
                        option { key: "{option.key()}", value: "{option.key()}", selected: option == condition.comparison(), "{option.label()}" }
                    }
                }
                output { class: "curation-select-rule-readout",
                    match thresholds.get(index) {
                        Some(threshold) => threshold.label(),
                        None => condition.value().to_string(),
                    }
                }
                if let Some(unit) = condition.fact().unit() {
                    span { "{unit}" }
                }
                if removable {
                    Button {
                        purpose: ActionPurpose::Remove,
                        variant: ButtonVariant::Quiet,
                        size: ButtonSize::Compact,
                        class: "curation-select-condition-remove",
                        title: "Remove this condition",
                        "aria-label": "Remove the {condition.fact().label()} condition",
                        onclick: {
                            let edit = edit.clone();
                            let conditions = conditions.clone();
                            move |_| {
                                let mut conditions = conditions.clone();
                                conditions.remove(index);
                                edit.set(Selection::Episodes { conditions });
                            }
                        },
                        CloseIcon {}
                    }
                }
            }
        }
        Button {
            purpose: ActionPurpose::Create,
            variant: ButtonVariant::Link,
            size: ButtonSize::Compact,
            title: "Keep only episodes that also meet another condition",
            onclick: move |_| edit.set(added.clone()),
            "Add condition"
        }
    }
}

/// "Keep moments progressing at least 0.89× the expert rate".
#[component]
fn ProgressRule(threshold: Option<Threshold>) -> Element {
    rsx! {
        span { "Keep moments progressing at least" }
        if let Some(threshold) = threshold {
            output { class: "curation-select-rule-readout", "{threshold.label()}" }
        }
        span { "the expert rate" }
    }
}

/// One category a Select may keep: its name, how many of the subtasks
/// reaching the Select it holds, and the hue the grouping gave it.
#[derive(Clone, Debug, PartialEq)]
struct CategoryChoice {
    name: String,
    members: usize,
    tint: String,
}

impl CategoryChoice {
    /// The categories of the grouping above, biggest first, as the grouping
    /// itself lists them.
    fn of(view: &RuleView) -> Vec<Self> {
        let (Population::Subtasks(rows), CurationNodeKind::GroupSimilarSubtasks(grouping)) =
            (&view.input, view.measure.kind())
        else {
            return Vec::new();
        };
        let hues = CategoryHues::of_rows(grouping.categories(), rows);
        let mut choices = grouping
            .categories()
            .iter()
            .map(|category| Self {
                name: category.name().to_owned(),
                members: rows
                    .iter()
                    .filter(|row| row.category() == Some(category.name()))
                    .count(),
                tint: hues.tint_of(Some(category.name())),
            })
            .collect::<Vec<_>>();
        choices.sort_by_key(|choice| std::cmp::Reverse(choice.members));
        choices
    }
}

/// "Keep subtasks in [Folding] [Pick and Place] …", one toggle per category.
#[component]
fn CategoryRule(edit: RuleEdit, names: Vec<String>, categories: Vec<CategoryChoice>) -> Element {
    rsx! {
        span { "Keep subtasks in" }
        if categories.is_empty() {
            span { class: "curation-select-rule-hint", "the categories the grouping above proposes" }
        }
        div { class: "curation-select-chips", role: "group", "aria-label": "Categories to keep",
            for category in categories {
                {
                    let chosen = names.contains(&category.name);
                    let toggled = {
                        let mut names = names.clone();
                        if chosen {
                            names.retain(|name| name != &category.name);
                        } else {
                            names.push(category.name.clone());
                        }
                        names
                    };
                    let edit = edit.clone();
                    rsx! {
                        button {
                            key: "{category.name}",
                            r#type: "button",
                            class: "curation-select-chip",
                            style: "{category.tint}",
                            "aria-pressed": "{chosen}",
                            onclick: move |_| edit.set(Selection::Categories { names: toggled.clone() }),
                            span { class: "curation-select-chip-name", "{category.name}" }
                            span { class: "curation-select-chip-count", "{category.members}" }
                        }
                    }
                }
            }
        }
    }
}
