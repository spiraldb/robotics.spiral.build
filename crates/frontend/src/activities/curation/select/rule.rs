//! The rule a Select keeps by, written as one sentence in the words of the
//! node above it, with the sentence's variable words as its fields.
use super::SelectView;
use crate::activities::curation::document::{
    Comparison, CurationNode, CurationNodeKind, EpisodeFact, Selection,
};
use crate::activities::curation::population::Population;
use crate::activities::curation::similar::CategoryHues;
use crate::activities::curation::state::CurationController;
use dioxus::prelude::*;
use spiral_robotics_ui::{Select, TextInput, TextInputKind};

/// One Select node, edited through the controller.
#[derive(Clone, PartialEq)]
struct SelectNode {
    id: String,
    controller: CurationController,
}

impl SelectNode {
    /// Replace the node's rule. The document refuses a rule for another
    /// basis, so a slip can never re-point a Select at something else.
    fn set(&self, rule: Selection) {
        let id = self.id.clone();
        let _ = self.controller.edit(move |payload| {
            match payload.node_mut(&id).map(CurationNode::kind_mut) {
                Some(CurationNodeKind::Select(selection)) if selection.basis() == rule.basis() => {
                    *selection = rule;
                    Ok(())
                }
                _ => Err("This node no longer selects by that measure.".to_owned()),
            }
        });
    }
}

#[component]
pub(super) fn SelectRule(controller: CurationController, view: SelectView) -> Element {
    let node = SelectNode {
        id: view.node.clone(),
        controller,
    };
    rsx! {
        div { class: "curation-select-rule",
            match view.selection.clone() {
                Selection::Episodes { fact, comparison, value } => rsx! {
                    EpisodeRule { node, fact, comparison, value }
                },
                Selection::Similar { minimum } => {
                    let query = match view.measure.kind() {
                        CurationNodeKind::FindSimilarSubtasks(search) => search.query().trim().to_owned(),
                        _ => String::new(),
                    };
                    rsx! { SimilarRule { node, minimum, query } }
                }
                Selection::Categories { names } => {
                    let categories = CategoryChoice::of(controller, &view);
                    rsx! { CategoryRule { node, names, categories } }
                }
                Selection::Progressing { minimum } => rsx! { ProgressRule { node, minimum } },
            }
        }
    }
}

/// "Keep episodes whose [duration] is [at least] [20] s".
#[component]
fn EpisodeRule(node: SelectNode, fact: EpisodeFact, comparison: Comparison, value: f64) -> Element {
    let with_fact = node.clone();
    let with_comparison = node.clone();
    rsx! {
        span { "Keep episodes whose" }
        Select {
            class: "curation-select-rule-choice",
            value: fact.key().to_owned(),
            "aria-label": "Recorded fact",
            onchange: move |key: String| {
                if let Some(fact) = EpisodeFact::from_key(&key) {
                    with_fact.set(Selection::Episodes { fact, comparison, value });
                }
            },
            for option in EpisodeFact::ALL {
                option { key: "{option.key()}", value: "{option.key()}", selected: option == fact, "{option.label()}" }
            }
        }
        span { "is" }
        Select {
            class: "curation-select-rule-choice",
            value: comparison.key().to_owned(),
            "aria-label": "Comparison",
            onchange: move |key: String| {
                if let Some(comparison) = Comparison::from_key(&key) {
                    with_comparison.set(Selection::Episodes { fact, comparison, value });
                }
            },
            for option in Comparison::ALL {
                option { key: "{option.key()}", value: "{option.key()}", selected: option == comparison, "{option.label()}" }
            }
        }
        ThresholdField {
            value,
            label: "Threshold".to_owned(),
            on_change: move |value| node.set(Selection::Episodes { fact, comparison, value }),
        }
        if let Some(unit) = fact.unit() {
            span { "{unit}" }
        }
    }
}

/// "Keep subtasks at least [50] % likely to be “fold the towel”".
#[component]
fn SimilarRule(node: SelectNode, minimum: f64, query: String) -> Element {
    rsx! {
        span { "Keep subtasks at least" }
        ThresholdField {
            value: (minimum * 100.0).round(),
            label: "Similarity, in percent".to_owned(),
            on_change: move |percent: f64| {
                node.set(Selection::Similar { minimum: (percent / 100.0).clamp(0.0, 1.0) });
            },
        }
        span {
            if query.is_empty() { "% likely to be what the search above asks" } else { "% likely to be “{query}”" }
        }
    }
}

/// "Keep moments progressing at least [0]".
#[component]
fn ProgressRule(node: SelectNode, minimum: f64) -> Element {
    rsx! {
        span { "Keep moments progressing at least" }
        ThresholdField {
            value: minimum,
            label: "Progress".to_owned(),
            on_change: move |minimum| node.set(Selection::Progressing { minimum }),
        }
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
    fn of(controller: CurationController, view: &SelectView) -> Vec<Self> {
        let Population::Subtasks { rows, .. } = &view.input else {
            return Vec::new();
        };
        controller.with_payload(|payload| {
            let Some(grouping) = payload.grouping_for(view.measure.id()) else {
                return Vec::new();
            };
            let hues = CategoryHues::of_rows(grouping, rows);
            let mut choices = grouping
                .categories()
                .iter()
                .map(|category| Self {
                    name: category.name().to_owned(),
                    members: rows
                        .iter()
                        .filter(|row| grouping.category_of(row) == Some(category.name()))
                        .count(),
                    tint: hues.tint_of(Some(category.name())),
                })
                .collect::<Vec<_>>();
            choices.sort_by_key(|choice| std::cmp::Reverse(choice.members));
            choices
        })
    }
}

/// "Keep subtasks in [Folding] [Pick and Place] …", one toggle per category.
#[component]
fn CategoryRule(node: SelectNode, names: Vec<String>, categories: Vec<CategoryChoice>) -> Element {
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
                    let node = node.clone();
                    rsx! {
                        button {
                            key: "{category.name}",
                            r#type: "button",
                            class: "curation-select-chip",
                            style: "{category.tint}",
                            "aria-pressed": "{chosen}",
                            onclick: move |_| node.set(Selection::Categories { names: toggled.clone() }),
                            span { class: "curation-select-chip-name", "{category.name}" }
                            span { class: "curation-select-chip-count", "{category.members}" }
                        }
                    }
                }
            }
        }
    }
}

/// A threshold typed as text. The text is the operator's until it reads as a
/// number, so "0." and "-" survive while they are being typed; a value
/// changed from elsewhere replaces it.
#[component]
fn ThresholdField(value: f64, label: String, on_change: EventHandler<f64>) -> Element {
    let mut text = use_signal(|| value.to_string());
    use_effect(use_reactive(&value, move |value| {
        let typed = text.peek().trim().parse::<f64>().ok();
        if typed != Some(value) {
            text.set(value.to_string());
        }
    }));
    rsx! {
        TextInput {
            class: "curation-select-rule-value",
            kind: TextInputKind::Decimal,
            value: text(),
            "aria-label": "{label}",
            oninput: move |typed: String| {
                if let Ok(number) = typed.trim().parse::<f64>()
                    && number.is_finite()
                {
                    on_change.call(number);
                }
                text.set(typed);
            },
        }
    }
}
