//! The recipe column: which dataset, the ordered steps that judge and shape
//! its episodes, and one place to add the next step. Every control writes
//! the same draft; the sheet answers on Run. Each step row mirrors its
//! pipeline card (the same kind mark, the same "n · Kind" title), then reads
//! as one sentence or one named operation.

use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationStep;
use spiral_robotics_common::curation::facts::{BuiltinFact, FactCatalog, FactKind, TopicRef};
use spiral_robotics_common::curation::rules::{
    ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition, TransformOperation,
};
use spiral_robotics_common::datasets::DatasetSelection;

use super::pipeline::{PipelineSelection, StageKind, StageMark};
use super::sheet::RuleTone;
use super::state::CurationController;
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::components::interaction::{
    Action, ActionPurpose, ActionVariant, ChevronIcon, CloseIcon, Toggle,
};
use crate::format::{GroupedCountPresentation, LongDurationPresentation};

const CAMERA_TOPICS_ID: &str = "curation-camera-topics";

#[component]
pub(super) fn RecipePanel(controller: CurationController) -> Element {
    let source = controller.spec().source().cloned();
    let cameras = use_resource(use_reactive(&source, move |source| async move {
        let Some(source) = source else {
            return Vec::new();
        };
        DatasetClient::current()
            .dataset_cameras(&source)
            .await
            .ok()
            .and_then(|value| value["cameras"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|camera| camera["topic"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    }));
    let camera_topics = cameras.read().clone().unwrap_or_default();
    rsx! {
        section { class: "curation-recipe", "aria-label": "Curation recipe",
            SourceSection { controller }
            OrderedRules { controller }
            super::rule_composer::RuleComposer { controller, camera_topics: camera_topics.clone() }
            datalist { id: CAMERA_TOPICS_ID,
                for topic in camera_topics { option { value: "{topic}" } }
            }
        }
    }
}

#[component]
fn OrderedRules(controller: CurationController) -> Element {
    let spec = controller.spec();
    let catalog = spec.catalog();
    let outcome = controller.outcome();
    let counts_current = controller
        .judged_spec()
        .is_some_and(|judged| spec.accepts_output_from(&judged));
    let steps = spec.steps();
    let reorder = super::reorder::RuleReorder::use_binding(controller);
    let selection = controller.pipeline_selection();
    let hovered = controller.hovered_rule();
    rsx! {
        section { class: "curation-section curation-steps", id: "curation-ordered-rules", "aria-label": "Recipe steps",
            div { class: "curation-rule-insertion", style: reorder.marker_style(), "aria-hidden": "true" }
            p { id: "curation-reorder-help", class: "sr-only", "Drag to reorder. Or press Space, then arrow keys to move, Space to drop, and Escape to cancel." }
            div { class: "sr-only", "aria-live": "polite", "{reorder.announcement()}" }
            div { class: "curation-section-title", span { "Steps" } span { "{steps.len()}" } }
            if steps.is_empty() {
                p { class: "curation-section-note", "Add a filter to exclude whole episodes, or a transform to change their training data." }
            }
            for (index, step) in steps.iter().enumerate() {
                {
                    let id = step.id().to_owned();
                    let is_filter = matches!(step, CurationStep::Filter(_));
                    let (kind, tone, count) = match step {
                        CurationStep::Filter(rule) => (
                            StageKind::Filter,
                            Some(RuleTone::of(&spec, rule.id())),
                            counts_current
                                .then(|| outcome.as_ref().and_then(|outcome| outcome.stage_impacts().iter().find(|impact| impact.id() == rule.id()).map(|impact| format!("{} excluded", impact.excluded()))))
                                .flatten(),
                        ),
                        CurationStep::Transform(transform) => (
                            StageKind::of_transform(transform.operation()),
                            None,
                            counts_current
                                .then(|| outcome.as_ref().and_then(|outcome| outcome.stage_impacts().iter().find(|impact| impact.id() == transform.id()).and_then(|impact| impact.affected_episodes()).map(|reached| format!("{reached} reached"))))
                                .flatten(),
                        ),
                    };
                    let select_id = id.clone();
                    let focus_id = id.clone();
                    let enter_id = id.clone();
                    let remove_id = id.clone();
                    rsx! {
                        article { key: "{id}", class: "curation-step", id: "curation-rule-{id}",
                            style: reorder.style(&id),
                            "data-dragging": reorder.picked(&id),
                            "data-selected": selection == PipelineSelection::Step(id.clone()),
                            "data-hovered": is_filter && hovered.as_deref() == Some(id.as_str()),
                            onclick: move |_| controller.select_pipeline(PipelineSelection::Step(select_id.clone())),
                            onfocusin: move |_| controller.select_pipeline(PipelineSelection::Step(focus_id.clone())),
                            // Resting the pointer on a filter lights the tiles it struck.
                            onmouseenter: move |_| if is_filter { controller.set_hovered_rule(Some(enter_id.clone())) },
                            onmouseleave: move |_| if is_filter { controller.set_hovered_rule(None) },
                            header { class: "curation-step-head",
                                StageMark { kind, tone }
                                h4 { class: "curation-step-title", "{index + 1} · " if is_filter { "Filter" } else { "Transform" } }
                                if let Some(count) = count {
                                    span { class: "curation-step-count", title: if is_filter { "Episodes this filter excluded in the visible run" } else { "Episodes this transform reached in the visible run" }, "{count}" }
                                }
                                div { class: "curation-step-tools",
                                    button { r#type: "button", class: "curation-reorder-handle", id: "curation-rule-handle-{id}",
                                        "aria-label": "Reorder step {index + 1}", "aria-describedby": "curation-reorder-help", "aria-pressed": reorder.picked(&id),
                                        onpointerdown: { let id = id.clone(); move |event| reorder.pointer_down(&id, event) },
                                        onpointermove: move |event| reorder.pointer_move(event),
                                        onpointerup: move |event| reorder.pointer_up(event),
                                        onpointercancel: move |event| reorder.pointer_cancel(event),
                                        onlostpointercapture: move |event| reorder.pointer_cancel(event),
                                        onkeydown: { let id = id.clone(); move |event| reorder.keyboard(&id, event) },
                                        title: "Drag to reorder · Space then arrow keys",
                                        svg { width: "16", height: "16", view_box: "0 0 16 16", fill: "currentColor", "aria-hidden": "true",
                                            for (x, y) in [(5,3),(11,3),(5,8),(11,8),(5,13),(11,13)] { circle { cx: "{x}", cy: "{y}", r: "1.2" } }
                                        }
                                    }
                                    Action { purpose: ActionPurpose::Remove, variant: ActionVariant::Icon,
                                        aria_label: "Remove step {index + 1}", title: "Remove step",
                                        onclick: move |event: Event<MouseData>| {
                                            event.stop_propagation();
                                            let id = remove_id.clone();
                                            controller.edit(move |spec| {
                                                if is_filter { spec.remove_rule(&id); } else { spec.remove_transform(&id); }
                                            });
                                            controller.select_pipeline(PipelineSelection::Source);
                                        },
                                        CloseIcon {}
                                    }
                                }
                            }
                            match step {
                                CurationStep::Filter(rule) => rsx! { RuleEditor { controller, rule: (*rule).clone(), catalog: catalog.clone() } },
                                CurationStep::Transform(transform) => rsx! { TransformEditor { controller, transform: (*transform).clone(), catalog: catalog.clone() } },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Which dataset the recipe judges, and how much of it there is. The name
/// field leads; when the catalog cannot be listed, the field still names the
/// dataset and one quiet line says why the list is missing.
#[component]
fn SourceSection(controller: CurationController) -> Element {
    let sessions = crate::shell::project::ProjectSessionSignals::current();
    let datasets = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
        move || sessions.catalog_available(),
        || DatasetClient::current().list_datasets(),
    );
    let source_label = use_memo(move || controller.spec().source().map(DatasetSelection::label));
    let facts = use_resource(move || {
        let source = source_label();
        async move {
            let source = source?;
            let sql = format!(
                "SELECT count(*) AS episodes, sum(duration_ns) AS duration_ns FROM spql.mcap.episodes('{}')",
                source.replace('\'', "''")
            );
            let outcome = DatasetClient::current().run_spiral_query(sql, 1).await;
            Some(match outcome {
                Ok(snapshot) => snapshot
                    .rows()
                    .first()
                    .map(|row| SourceFacts::from_row(row).label())
                    .ok_or_else(|| "The dataset returned no totals.".to_owned()),
                Err(error) => Err(error.to_string()),
            })
        }
    });
    let mut typed = use_signal(String::new);
    // The typed field carries the saved dataset name, so the bar's chip is
    // the only other place the name appears.
    use_effect(move || {
        if let Some(label) = source_label()
            && typed.peek().is_empty()
        {
            typed.set(label);
        }
    });
    let choose = move |label: String| {
        let selection = DatasetSelection::new(label.split('.').map(str::to_owned).collect());
        controller.edit(|spec| spec.set_source(selection.ok()));
    };
    let response = datasets.read();
    let current = source_label();
    rsx! {
        div { class: "curation-section curation-source-section",
            "data-selected": controller.pipeline_selection() == PipelineSelection::Source,
            onclick: move |_| controller.select_pipeline(PipelineSelection::Source),
            div { class: "curation-section-title", span { "Dataset" } }
            match response.as_ref() {
                Some(Ok(items)) => rsx! {
                    label { r#for: "curation-source", class: "sr-only", "Dataset" }
                    select {
                        id: "curation-source", class: "curation-select w-full",
                        value: current.clone().unwrap_or_default(),
                        onchange: move |event| choose(event.value()),
                        option { value: "", disabled: true, selected: current.is_none(),
                            if items.is_empty() { "No datasets available" } else { "Choose a dataset" } }
                        // A saved recipe may name a dataset the catalog no longer lists.
                        if let Some(label) = current.as_ref().filter(|label| !items.iter().any(|item| item.selection().label() == **label)) {
                            option { value: "{label}", selected: true, "{label} (not in catalog)" }
                        }
                        for item in items {
                            option { value: "{item.selection().label()}", selected: current.as_deref() == Some(item.selection().label().as_str()), "{item.selection().label()}" }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    form { class: "curation-inline-form",
                        onsubmit: move |event| { event.prevent_default(); let name = typed().trim().to_owned(); if !name.is_empty() { choose(name); } },
                        input { class: "curation-input", "aria-label": "Dataset name", placeholder: "dataset_name",
                            value: "{typed}", oninput: move |event| typed.set(event.value()),
                            onkeydown: move |event| if event.key() == Key::Enter {
                                event.prevent_default();
                                let name = typed().trim().to_owned();
                                if !name.is_empty() { choose(name); }
                            } }
                        Action { purpose: ActionPurpose::Apply, variant: ActionVariant::Outline, r#type: "submit",
                            disabled: typed().trim().is_empty() || current.as_deref() == Some(typed().trim()), "Use" }
                    }
                    p { class: "curation-source-caution", role: "status", title: "{error}",
                        "Dataset list unavailable · type its catalog name" }
                },
                None => rsx! { p { class: "curation-section-note", role: "status", "Loading datasets…" } },
            }
            match facts.read().as_ref() {
                Some(Some(Ok(label))) => rsx! { p { class: "curation-section-note curation-source-facts", "{label}" } },
                Some(Some(Err(error))) => rsx! { p { class: "curation-section-note is-alert", role: "alert", "{error}" } },
                Some(None) => rsx! { p { class: "curation-section-note", "Every episode is judged; nothing is copied or changed." } },
                None => rsx! { p { class: "curation-section-note", role: "status", "Counting episodes…" } },
            }
        }
    }
}

/// Totals over the chosen dataset, from one aggregate query.
struct SourceFacts {
    episodes: Option<u64>,
    duration_ns: Option<u64>,
}

impl SourceFacts {
    fn from_row(row: &serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            episodes: row.get("episodes").and_then(serde_json::Value::as_u64),
            duration_ns: row.get("duration_ns").and_then(serde_json::Value::as_u64),
        }
    }
    fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(episodes) = self.episodes {
            parts.push(format!("{} episodes", episodes.grouped_label()));
        }
        if let Some(duration) = self.duration_ns {
            parts.push(format!(
                "{} recorded",
                (duration as f64 / 1e9).long_duration_label()
            ));
        }
        if parts.is_empty() {
            "No episode totals available.".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

/// A filter reads as one sentence: exclude when fact · comparison · value.
#[component]
fn RuleEditor(controller: CurationController, rule: CurationRule, catalog: FactCatalog) -> Element {
    let id = rule.id().to_owned();
    let problem = rule.condition().validate(&catalog).err();
    rsx! {
        div { class: "curation-step-body",
            div { class: "curation-sentence",
                span { class: "curation-rule-word", "Exclude when" }
                ConditionEditor {
                    catalog: catalog.clone(),
                    condition: rule.condition().clone(),
                    on_change: move |condition: RuleCondition| {
                        let id = id.clone();
                        controller.edit(move |spec| if let Some(rule) = spec.rule_mut(&id) { *rule.condition_mut() = condition; });
                    },
                }
            }
            if let Some(problem) = problem {
                p { class: "curation-rule-invalid", role: "status", "This rule {problem}." }
            }
        }
    }
}

/// `fact · operator · value`, with the operators that fit the fact's kind.
#[component]
fn ConditionEditor(
    catalog: FactCatalog,
    condition: RuleCondition,
    on_change: EventHandler<RuleCondition>,
) -> Element {
    let kind = catalog
        .entry(condition.fact())
        .map_or(FactKind::Number, |entry| entry.kind());
    let mut text = use_signal(|| condition.value().display());
    let mut fact_change = {
        let condition = condition.clone();
        let catalog = catalog.clone();
        move |fact: String| {
            let mut next = condition.clone();
            let next_kind = catalog
                .entry(&fact)
                .map_or(FactKind::Number, |entry| entry.kind());
            next.set_fact(fact);
            if !next.op().applies_to(next_kind) {
                next.set_op(ConditionOp::for_kind(next_kind)[0]);
            }
            if next_kind != kind {
                next.set_value(ConditionValue::None);
                text.set(String::new());
            }
            on_change.call(next);
        }
    };
    let op_change = {
        let condition = condition.clone();
        move |key: String| {
            if let Some(op) = ConditionOp::from_key(&key) {
                let mut next = condition.clone();
                next.set_op(op);
                on_change.call(next);
            }
        }
    };
    let mut value_change = {
        let condition = condition.clone();
        move |raw: String| {
            text.set(raw.clone());
            let mut next = condition.clone();
            next.set_value(ConditionValue::parse(kind, &raw).unwrap_or(ConditionValue::Text(raw)));
            on_change.call(next);
        }
    };
    rsx! {
        select { class: "curation-select curation-condition-fact", "aria-label": "Fact",
            value: "{condition.fact()}",
            onchange: move |event| fact_change(event.value()),
            optgroup { label: "Every episode",
                for entry in catalog.entries().iter().filter(|entry| entry.builtin()) {
                    option { value: "{entry.key()}", selected: entry.key() == condition.fact(), "{entry.label()}" }
                }
            }
            if catalog.entries().iter().any(|entry| !entry.builtin()) {
                optgroup { label: "Measurements",
                    for entry in catalog.entries().iter().filter(|entry| !entry.builtin()) {
                        option { value: "{entry.key()}", selected: entry.key() == condition.fact(), "{entry.label()}" }
                    }
                }
            }
            if catalog.entry(condition.fact()).is_none() {
                option { value: "{condition.fact()}", selected: true, "{condition.fact()} (removed)" }
            }
        }
        // The comparison and its value stay together, so a narrow column
        // breaks the sentence before "is below 10", never inside it.
        span { class: "curation-condition-test",
            select { class: "curation-select curation-condition-op", "aria-label": "Comparison",
                value: "{condition.op().key()}",
                onchange: move |event| op_change(event.value()),
                for op in ConditionOp::for_kind(kind) {
                    option { value: "{op.key()}", selected: *op == condition.op(), "{op.label()}" }
                }
            }
            if condition.op().needs_value() {
                input {
                    class: if kind == FactKind::Number { "curation-input curation-condition-value curation-mono" } else { "curation-input curation-condition-value" },
                    "aria-label": "Value",
                    inputmode: if kind == FactKind::Number { "decimal" } else { "text" },
                    placeholder: if kind == FactKind::Number { "0" } else { "text" },
                    value: "{text}",
                    oninput: move |event| value_change(event.value()),
                }
            }
        }
    }
}

/// A transform reads as its name over what it does; its fields open in
/// place when there is something to set.
#[component]
fn TransformEditor(
    controller: CurationController,
    transform: CurationTransform,
    catalog: FactCatalog,
) -> Element {
    let id = transform.id().to_owned();
    let problem = transform.validate(&catalog).err();
    let mut expanded = use_signal(|| problem.is_some());
    let set_operation = {
        let id = id.clone();
        move |operation: TransformOperation| {
            let id = id.clone();
            controller.edit(move |spec| {
                if let Some(transform) = spec.transform_mut(&id) {
                    transform.set_operation(operation);
                }
            });
        }
    };
    let condition_id = id.clone();
    let only_when_id = id.clone();
    let label_id = id.clone();
    let summary = transform.operation().summary();
    let kind_label = transform.operation().kind_label();
    let progress_transform = matches!(
        transform.operation(),
        TransformOperation::SelectProgressChunks { .. }
    );
    // The built-in policy has one name; other transforms lead with what the
    // user called them, else with what they do.
    let (name, detail) = if progress_transform {
        (kind_label.to_owned(), None)
    } else if transform.label().trim().is_empty() {
        (summary.clone(), Some(kind_label.to_owned()))
    } else {
        (transform.label().to_owned(), Some(summary.clone()))
    };
    rsx! {
        div { class: "curation-step-body curation-transform", "data-expanded": expanded(),
            Action {
                purpose: ActionPurpose::Toggle, variant: ActionVariant::Quiet, class: "curation-transform-summary",
                aria_expanded: expanded(), title: if expanded() { "Hide the settings" } else { "Show the settings" },
                onclick: move |_| expanded.toggle(),
                ChevronIcon { open: expanded() }
                span { class: "curation-transform-copy",
                    span { class: "curation-transform-name", "{name}" }
                    if let Some(detail) = detail { span { class: "curation-transform-detail", "{detail}" } }
                }
            }
            if expanded() {
                div { class: "curation-transform-fields",
                    if !progress_transform {
                        div { class: "curation-field-grid",
                            label { r#for: "{transform.id()}-label", "Name" }
                            input {
                                id: "{transform.id()}-label",
                                class: "curation-input curation-transform-label",
                                placeholder: "{summary}",
                                value: "{transform.label()}",
                                oninput: move |event| {
                                    let id = label_id.clone();
                                    let label = event.value();
                                    controller.edit(move |spec| if let Some(transform) = spec.transform_mut(&id) { transform.set_label(label); });
                                },
                            }
                        }
                    }
                    div { class: "curation-field-grid",
                        match transform.operation().clone() {
                            TransformOperation::SelectProgressChunks { selection } => rsx! {
                                super::progress::ProgressStatus { selection }
                            },
                            TransformOperation::CameraColorProfile { topic, profile } => rsx! {
                                label { r#for: "{transform.id()}-topic", "Camera" }
                                input { id: "{transform.id()}-topic", class: "curation-input", list: CAMERA_TOPICS_ID, placeholder: "/top-camera",
                                    value: "{topic.topic()}",
                                    oninput: { let set_operation = set_operation.clone(); let profile = profile.clone();
                                        move |event| set_operation(TransformOperation::CameraColorProfile { topic: TopicRef::new(event.value()), profile: profile.clone() }) } }
                                label { r#for: "{transform.id()}-profile", "Profile" }
                                select { id: "{transform.id()}-profile", class: "curation-select", value: "{profile}",
                                    onchange: { let set_operation = set_operation.clone(); let topic = topic.clone();
                                        move |event| set_operation(TransformOperation::CameraColorProfile { topic: topic.clone(), profile: event.value() }) },
                                    for option in TransformOperation::COLOR_PROFILES {
                                        option { value: "{option}", selected: option == profile, "{option}" }
                                    }
                                }
                            },
                            TransformOperation::ResizeCamera { topic, width, height } => rsx! {
                                label { r#for: "{transform.id()}-topic", "Camera" }
                                input { id: "{transform.id()}-topic", class: "curation-input", list: CAMERA_TOPICS_ID, placeholder: "/top-camera",
                                    value: "{topic.topic()}",
                                    oninput: { let set_operation = set_operation.clone();
                                        move |event| set_operation(TransformOperation::ResizeCamera { topic: TopicRef::new(event.value()), width, height }) } }
                                label { r#for: "{transform.id()}-size", "Size" }
                                div { class: "curation-inline-form",
                                    input { id: "{transform.id()}-size", class: "curation-input curation-mono curation-dimension", "aria-label": "Width", inputmode: "numeric",
                                        value: "{width}",
                                        oninput: { let set_operation = set_operation.clone(); let topic = topic.clone();
                                            move |event| if let Ok(width) = event.value().trim().parse::<u32>() { set_operation(TransformOperation::ResizeCamera { topic: topic.clone(), width, height }) } } }
                                    span { class: "curation-rule-word", "×" }
                                    input { class: "curation-input curation-mono curation-dimension", "aria-label": "Height", inputmode: "numeric",
                                        value: "{height}",
                                        oninput: { let set_operation = set_operation.clone(); let topic = topic.clone();
                                            move |event| if let Ok(height) = event.value().trim().parse::<u32>() { set_operation(TransformOperation::ResizeCamera { topic: topic.clone(), width, height }) } } }
                                }
                            },
                            TransformOperation::TrimTime { start_seconds, end_seconds } => rsx! {
                                label { r#for: "{transform.id()}-start", "From (s)" }
                                input { id: "{transform.id()}-start", class: "curation-input curation-mono", inputmode: "decimal",
                                    value: "{start_seconds}",
                                    oninput: { let set_operation = set_operation.clone();
                                        move |event| if let Ok(start) = event.value().trim().parse::<f64>() { set_operation(TransformOperation::TrimTime { start_seconds: start, end_seconds }) } } }
                                label { r#for: "{transform.id()}-end", "To (s)" }
                                input { id: "{transform.id()}-end", class: "curation-input curation-mono", inputmode: "decimal", placeholder: "end of episode",
                                    value: end_seconds.map(|end| end.to_string()).unwrap_or_default(),
                                    oninput: { let set_operation = set_operation.clone();
                                        move |event| { let raw = event.value(); let end = raw.trim(); if end.is_empty() { set_operation(TransformOperation::TrimTime { start_seconds, end_seconds: None }) } else if let Ok(end) = end.parse::<f64>() { set_operation(TransformOperation::TrimTime { start_seconds, end_seconds: Some(end) }) } } } }
                            },
                        }
                    }
                    if !progress_transform || transform.condition().is_some() {
                        div { class: "curation-sentence",
                            Toggle {
                                label: "Only when".to_owned(),
                                checked: transform.condition().is_some(),
                                onchange: move |only_when: bool| {
                                    let id = only_when_id.clone();
                                    controller.edit(move |spec| if let Some(transform) = spec.transform_mut(&id) {
                                        transform.set_condition(only_when.then(|| RuleCondition::new(BuiltinFact::DurationSeconds.key(), ConditionOp::AtLeast, ConditionValue::None)));
                                    });
                                },
                            }
                            if let Some(condition) = transform.condition().cloned() {
                                ConditionEditor {
                                    catalog: catalog.clone(),
                                    condition,
                                    on_change: move |condition: RuleCondition| {
                                        let id = condition_id.clone();
                                        controller.edit(move |spec| if let Some(transform) = spec.transform_mut(&id) { transform.set_condition(Some(condition)); });
                                    },
                                }
                            } else {
                                span { class: "curation-rule-word", "every kept episode" }
                            }
                        }
                    }
                }
            }
            if let Some(problem) = problem {
                p { class: "curation-rule-invalid", role: "status", "This transform {problem}." }
            }
        }
    }
}
