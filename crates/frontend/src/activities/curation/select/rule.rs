//! The rule a Select keeps by, written as one sentence in the words of the
//! node above it, with the sentence's variable words as its fields.
use super::SelectView;
use super::strip::ThresholdScale;
use crate::activities::curation::document::{
    Comparison, CurationNode, CurationNodeKind, EpisodeFact, Selection,
};
use crate::activities::curation::population::Population;
use crate::activities::curation::similar::CategoryHues;
use crate::activities::curation::state::CurationController;
use dioxus::prelude::*;
use spiral_robotics_ui::Select;

/// One Select node, edited through the controller.
#[derive(Clone, PartialEq)]
struct SelectNode {
    id: String,
    controller: CurationController,
}

impl SelectNode {
    fn new(id: String, controller: CurationController) -> Self {
        Self { id, controller }
    }
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

/// Where a numeric rule's threshold stands, and the axis it slides along.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Threshold {
    value: f64,
    scale: ThresholdScale,
}

#[component]
pub(super) fn SelectRule(
    controller: CurationController,
    view: SelectView,
    /// The axis a numeric rule slides along, shared with the strip below it.
    scale: Option<ThresholdScale>,
) -> Element {
    let node = SelectNode::new(view.node.clone(), controller);
    let at = move |value: f64| scale.map(|scale| Threshold { value, scale });
    rsx! {
        div { class: "curation-select-rule",
            match view.selection.clone() {
                Selection::Episodes { fact, comparison, value } => rsx! {
                    if let Some(threshold) = at(value) {
                        EpisodeRule { node, fact, comparison, threshold }
                    }
                },
                Selection::Similar { minimum } => {
                    let query = match view.measure.kind() {
                        CurationNodeKind::FindSimilarSubtasks(search) => search.query().trim().to_owned(),
                        _ => String::new(),
                    };
                    rsx! {
                        if let Some(threshold) = at(minimum) {
                            SimilarRule { node, threshold, query }
                        }
                    }
                }
                Selection::Categories { names } => {
                    let categories = CategoryChoice::of(controller, &view);
                    rsx! { CategoryRule { node, names, categories } }
                }
                Selection::Progressing { minimum } => rsx! {
                    if let Some(threshold) = at(minimum) {
                        ProgressRule { node, threshold }
                    }
                },
            }
        }
    }
}

/// "Keep episodes whose [duration] is [at least] [——●——] 20 s".
#[component]
fn EpisodeRule(
    node: SelectNode,
    fact: EpisodeFact,
    comparison: Comparison,
    threshold: Threshold,
) -> Element {
    let value = threshold.value;
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
        ThresholdSlider {
            threshold,
            label: format!("Least {}", fact.label()),
            on_change: move |value| node.set(Selection::Episodes { fact, comparison, value }),
        }
        if let Some(unit) = fact.unit() {
            span { "{unit}" }
        }
    }
}

/// "Keep subtasks at least [——●——] 50% likely to be “fold the towel”".
#[component]
fn SimilarRule(node: SelectNode, threshold: Threshold, query: String) -> Element {
    rsx! {
        span { "Keep subtasks at least" }
        ThresholdSlider {
            threshold,
            label: "Least similarity".to_owned(),
            on_change: move |minimum: f64| node.set(Selection::Similar { minimum }),
        }
        span {
            if query.is_empty() { "likely to be what the search above asks" } else { "likely to be “{query}”" }
        }
    }
}

/// "Keep moments progressing at least [——●——] 0.89".
#[component]
fn ProgressRule(node: SelectNode, threshold: Threshold) -> Element {
    rsx! {
        span { "Keep moments progressing at least" }
        ThresholdSlider {
            threshold,
            label: "Least progress".to_owned(),
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

/// A rule's threshold as a slider along the axis of the strip beneath it,
/// with the value it stands at beside it. The rule re-divides kept from left
/// out as it moves, and the arrows step it one step at a time.
#[component]
fn ThresholdSlider(threshold: Threshold, label: String, on_change: EventHandler<f64>) -> Element {
    let Threshold { value, scale } = threshold;
    rsx! {
        span { class: "curation-select-rule-slider",
            input {
                r#type: "range",
                min: "{scale.low()}",
                max: "{scale.high()}",
                step: "{scale.step()}",
                value: "{value}",
                "aria-label": "{label}",
                "aria-valuetext": "{scale.label(value)}",
                oninput: move |event: FormEvent| {
                    if let Ok(slid) = event.value().parse::<f64>() {
                        on_change.call(scale.snap(slid));
                    }
                },
            }
            output { class: "curation-select-rule-readout", "{scale.label(value)}" }
        }
    }
}
