//! The recipe column: which dataset, what is measured about each episode,
//! the rules that exclude, and the transforms the kept episodes receive.
//! Every control writes the same draft; the sheet answers on Run.

use dioxus::prelude::*;
use spiral_robotics_common::curation::CurationSpec;
use spiral_robotics_common::curation::facts::{
    BuiltinFact, CameraSample, CurationFact, FactCatalog, FactKind, FactMeasure, FieldAggregate,
    TopicRef,
};
use spiral_robotics_common::curation::rules::{
    ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition, TransformOperation,
};
use spiral_robotics_common::datasets::DatasetSelection;

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
    rsx! {
        section { class: "curation-recipe", "aria-label": "Curation recipe",
            SourceSection { controller }
            MeasurementsSection { controller }
            RulesSection { controller }
            TransformsSection { controller }
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
        div { class: "curation-section",
            div { class: "curation-section-title", span { "Source" } }
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

/// The measurements a rule may name beyond the five every episode carries
/// (duration, messages, channels, file size, source path), which the rule's
/// fact list offers under "Every episode".
#[component]
fn MeasurementsSection(controller: CurationController) -> Element {
    let spec = controller.spec();
    let source = spec.source().cloned();
    let camera_topics = use_resource(move || {
        let source = source.clone();
        async move {
            let source = source?;
            DatasetClient::current()
                .dataset_cameras(&source)
                .await
                .ok()
                .map(|value| CameraTopics::from_response(&value))
        }
    });
    let topics = camera_topics.read().clone().flatten().unwrap_or_default();
    rsx! {
        div { class: "curation-section",
            div { class: "curation-section-title",
                span { "Measurements" }
                span { class: "curation-section-count", "{spec.facts().len()}" }
            }
            datalist { id: CAMERA_TOPICS_ID,
                for topic in topics.topics.iter() { option { value: "{topic}" } }
            }
            for fact in spec.facts().iter().cloned() {
                FactEditor { key: "{fact.key()}", controller, fact }
            }
            label { r#for: "curation-add-measurement", class: "sr-only", "Add a measurement" }
            select {
                id: "curation-add-measurement", class: "curation-select curation-add",
                value: "",
                onchange: move |event| {
                    let kind = event.value();
                    controller.edit(|spec| NewFact::add(spec, &kind));
                },
                option { value: "", disabled: true, selected: true, "Add a measurement…" }
                option { value: "camera_brightness", "Camera brightness — how bright the sampled frames are" }
                option { value: "camera_score", "Camera score — how well the frames match a prompt" }
                option { value: "topic_field", "Topic field — a recorded value, such as a success flag" }
                option { value: "sql", "SQL — any read-only query returning id and value" }
            }
        }
    }
}

/// Camera topics the dataset actually recorded, offered as suggestions.
#[derive(Clone, Default, PartialEq)]
struct CameraTopics {
    topics: Vec<String>,
}

impl CameraTopics {
    fn from_response(value: &serde_json::Value) -> Self {
        Self {
            topics: value["cameras"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|camera| camera["topic"].as_str().map(str::to_owned))
                .collect(),
        }
    }
}

/// A freshly added measurement: a unique key from its kind, sensible defaults.
struct NewFact;

impl NewFact {
    fn add(spec: &mut CurationSpec, kind: &str) {
        let (base, label, measure) = match kind {
            "camera_brightness" => (
                "brightness",
                "Camera brightness",
                FactMeasure::CameraBrightness {
                    camera: CameraSample::new(TopicRef::new("")),
                },
            ),
            "camera_score" => (
                "score",
                "Camera score",
                FactMeasure::CameraScore {
                    camera: CameraSample::new(TopicRef::new("")),
                    prompt: String::new(),
                },
            ),
            "topic_field" => (
                "field",
                "Topic field",
                FactMeasure::TopicField {
                    topic: TopicRef::new(""),
                    field: String::new(),
                    aggregate: FieldAggregate::First,
                    value: FactKind::Number,
                },
            ),
            "sql" => (
                "custom",
                "Custom SQL",
                FactMeasure::Sql {
                    sql: String::new(),
                    value: FactKind::Number,
                },
            ),
            _ => return,
        };
        let mut key = base.to_owned();
        let mut ordinal = 2;
        while spec.facts().iter().any(|fact| fact.key() == key) {
            key = format!("{base}_{ordinal}");
            ordinal += 1;
        }
        spec.add_fact(CurationFact::new(key, label, measure));
    }
}

#[component]
fn FactEditor(controller: CurationController, fact: CurationFact) -> Element {
    let key = fact.key().to_owned();
    let problem = fact.validate().err();
    // A measurement that cannot run yet opens itself; a complete one rests as one line.
    let mut expanded = use_signal(|| problem.is_some());
    let replace = {
        let key = key.clone();
        move |next: CurationFact| {
            let key = key.clone();
            controller.edit(move |spec| spec.replace_fact(&key, next));
        }
    };
    let with_measure = {
        let fact = fact.clone();
        move |measure: FactMeasure| {
            let mut next = fact.clone();
            next.set_measure(measure);
            next
        }
    };
    let remove_key = key.clone();
    let summary = fact.measure().summary();
    rsx! {
        div { class: "curation-fact", "data-expanded": expanded(),
            div { class: "curation-fact-summary",
                Action {
                    purpose: ActionPurpose::Toggle, variant: ActionVariant::Quiet, class: "curation-fact-disclosure",
                    aria_expanded: expanded(), title: "{fact.measure().kind_label()} · {summary}",
                    onclick: move |_| expanded.toggle(),
                    ChevronIcon { open: expanded() }
                    span { class: "curation-fact-summary-label", "{fact.label()}" }
                    span { class: "curation-fact-detail", "{fact.measure().detail()}" }
                }
                Action {
                    purpose: ActionPurpose::Remove, variant: ActionVariant::Icon,
                    aria_label: "Remove measurement {fact.label()}", title: "Remove measurement",
                    onclick: move |_| { let key = remove_key.clone(); controller.edit(move |spec| spec.remove_fact(&key)); },
                    CloseIcon {}
                }
            }
            if expanded() {
                input {
                    class: "curation-input curation-fact-label",
                    "aria-label": "Measurement label",
                    title: "The name rules and the sheet use for this measurement",
                    value: "{fact.label()}",
                    oninput: {
                        let fact = fact.clone();
                        let replace = replace.clone();
                        move |event| { let mut next = fact.clone(); next.set_label(event.value()); replace(next); }
                    },
                }
                div { class: "curation-field-grid",
                    match fact.measure().clone() {
                        FactMeasure::CameraBrightness { camera } => rsx! {
                            CameraFields { camera: camera.clone(), on_change: {
                                let replace = replace.clone(); let with_measure = with_measure.clone();
                                move |camera| replace(with_measure(FactMeasure::CameraBrightness { camera }))
                            } }
                        },
                        FactMeasure::CameraScore { camera, prompt } => rsx! {
                            CameraFields { camera: camera.clone(), on_change: {
                                let replace = replace.clone(); let with_measure = with_measure.clone(); let prompt = prompt.clone();
                                move |camera| replace(with_measure(FactMeasure::CameraScore { camera, prompt: prompt.clone() }))
                            } }
                            label { r#for: "{fact.key()}-prompt", "Prompt" }
                            input { id: "{fact.key()}-prompt", class: "curation-input", placeholder: "a dimly lit workspace",
                                value: "{prompt}",
                                oninput: {
                                    let replace = replace.clone(); let with_measure = with_measure.clone(); let camera = camera.clone();
                                    move |event| replace(with_measure(FactMeasure::CameraScore { camera: camera.clone(), prompt: event.value() }))
                                } }
                        },
                        FactMeasure::TopicField { topic, field, aggregate, value } => rsx! {
                            label { r#for: "{fact.key()}-topic", "Topic" }
                            input { id: "{fact.key()}-topic", class: "curation-input", placeholder: "/studio/episode",
                                value: "{topic.topic()}",
                                oninput: {
                                    let replace = replace.clone(); let with_measure = with_measure.clone(); let topic = topic.clone(); let field = field.clone();
                                    move |event| replace(with_measure(FactMeasure::TopicField {
                                        topic: TopicRef::new(event.value()).with_variant(topic.variant().map(str::to_owned)),
                                        field: field.clone(), aggregate, value }))
                                } }
                            label { r#for: "{fact.key()}-field", "Field" }
                            input { id: "{fact.key()}-field", class: "curation-input curation-mono", placeholder: "discarded",
                                value: "{field}",
                                oninput: {
                                    let replace = replace.clone(); let with_measure = with_measure.clone(); let topic = topic.clone();
                                    move |event| replace(with_measure(FactMeasure::TopicField { topic: topic.clone(), field: event.value(), aggregate, value }))
                                } }
                            label { r#for: "{fact.key()}-aggregate", "Per episode" }
                            select { id: "{fact.key()}-aggregate", class: "curation-select", value: "{aggregate.key()}",
                                onchange: {
                                    let replace = replace.clone(); let with_measure = with_measure.clone(); let topic = topic.clone(); let field = field.clone();
                                    move |event| if let Some(aggregate) = FieldAggregate::from_key(&event.value()) {
                                        replace(with_measure(FactMeasure::TopicField { topic: topic.clone(), field: field.clone(), aggregate, value }))
                                    }
                                },
                                for option in FieldAggregate::ALL {
                                    option { value: "{option.key()}", selected: option == aggregate, "{option.label()} value" }
                                }
                            }
                            label { r#for: "{fact.key()}-kind", "Compares as" }
                            KindSelect { id: "{fact.key()}-kind", kind: value, on_change: {
                                let replace = replace.clone(); let with_measure = with_measure.clone(); let topic = topic.clone(); let field = field.clone();
                                move |value| replace(with_measure(FactMeasure::TopicField { topic: topic.clone(), field: field.clone(), aggregate, value }))
                            } }
                        },
                        FactMeasure::Sql { sql, value } => rsx! {
                            label { r#for: "{fact.key()}-sql", "SQL" }
                            textarea { id: "{fact.key()}-sql", class: "curation-textarea", rows: "4", spellcheck: "false",
                                placeholder: "SELECT id, value FROM …",
                                value: "{sql}",
                                oninput: {
                                    let replace = replace.clone(); let with_measure = with_measure.clone();
                                    move |event| replace(with_measure(FactMeasure::Sql { sql: event.value(), value }))
                                } }
                            label { r#for: "{fact.key()}-kind", "Compares as" }
                            KindSelect { id: "{fact.key()}-kind", kind: value, on_change: {
                                let replace = replace.clone(); let with_measure = with_measure.clone(); let sql = sql.clone();
                                move |value| replace(with_measure(FactMeasure::Sql { sql: sql.clone(), value }))
                            } }
                        },
                    }
                }
            }
            if let Some(problem) = problem {
                p { class: "curation-rule-invalid", role: "status", "{problem}" }
            }
        }
    }
}

#[component]
fn CameraFields(camera: CameraSample, on_change: EventHandler<CameraSample>) -> Element {
    let frames_text = camera
        .frames()
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let topic = camera.topic().clone();
    let frames = camera.frames().to_vec();
    let topic_id =
        use_hook(|| format!("camera-topic-{}", dioxus::dioxus_core::current_scope_id().0));
    let frames_id = format!("{topic_id}-frames");
    rsx! {
        label { r#for: "{topic_id}", "Camera" }
        input { id: "{topic_id}", class: "curation-input", list: CAMERA_TOPICS_ID, placeholder: "/top-camera",
            value: "{topic.topic()}",
            oninput: {
                let frames = frames.clone(); let topic = topic.clone();
                move |event| on_change.call(
                    CameraSample::new(TopicRef::new(event.value()).with_variant(topic.variant().map(str::to_owned)))
                        .with_frames(frames.clone()))
            } }
        label { r#for: "{frames_id}", title: "Zero-based frame positions in each episode's video", "Frames" }
        input { id: "{frames_id}", class: "curation-input curation-mono", placeholder: "0, 30",
            value: "{frames_text}",
            oninput: {
                let topic = topic.clone();
                move |event| {
                    let frames = event.value().split(',').filter_map(|part| part.trim().parse::<u32>().ok()).collect::<Vec<_>>();
                    on_change.call(CameraSample::new(topic.clone()).with_frames(frames));
                }
            } }
    }
}

#[component]
fn KindSelect(id: String, kind: FactKind, on_change: EventHandler<FactKind>) -> Element {
    rsx! {
        select { id: "{id}", class: "curation-select", value: "{kind.label()}",
            onchange: move |event| on_change.call(if event.value() == "text" { FactKind::Text } else { FactKind::Number }),
            option { value: "number", selected: kind == FactKind::Number, "a number" }
            option { value: "text", selected: kind == FactKind::Text, "text" }
        }
    }
}

/// If this, then exclude. Rules read top down; every firing rule is named on
/// the episode it removed.
#[component]
fn RulesSection(controller: CurationController) -> Element {
    let spec = controller.spec();
    let catalog = spec.catalog();
    let outcome = controller.outcome();
    let judged = controller.judged_spec();
    let counts_current = judged
        .as_ref()
        .is_some_and(|judged| spec.accepts_output_from(judged));
    rsx! {
        div { class: "curation-section",
            div { class: "curation-section-title",
                span { "Rules" }
                span { class: "curation-section-count", "{spec.rules().len()}" }
            }
            if spec.rules().is_empty() {
                p { class: "curation-section-note", "Every episode is kept until a rule says otherwise." }
            }
            for (index, rule) in spec.rules().iter().cloned().enumerate() {
                RuleEditor {
                    key: "{rule.id()}",
                    controller,
                    rule,
                    tone: RuleTone::at(index),
                    catalog: catalog.clone(),
                    removed: counts_current.then(|| outcome.as_ref().map(|outcome| outcome.rule_count(spec.rules()[index].id()))).flatten(),
                }
            }
            Action {
                purpose: ActionPurpose::Create, variant: ActionVariant::Outline, class: "curation-add-action",
                onclick: move |_| controller.edit(|spec| {
                    let id = format!("rule-{}-{:x}", js_sys::Date::now() as u64, (js_sys::Math::random() * u32::MAX as f64) as u64);
                    spec.add_rule(CurationRule::new(id, "", RuleCondition::new(BuiltinFact::DurationSeconds.key(), ConditionOp::Below, ConditionValue::None)));
                }),
                "Add rule"
            }
        }
    }
}

#[component]
fn RuleEditor(
    controller: CurationController,
    rule: CurationRule,
    tone: RuleTone,
    catalog: FactCatalog,
    removed: Option<u64>,
) -> Element {
    let id = rule.id().to_owned();
    let hovered = controller.hovered_rule().as_deref() == Some(rule.id());
    let problem = rule.condition().validate(&catalog).err();
    let toggle_id = id.clone();
    let label_id = id.clone();
    let remove_id = id.clone();
    let condition_id = id.clone();
    let handle = RuleHandle::new(controller, id);
    rsx! {
        div {
            class: "curation-rule",
            "data-enabled": rule.enabled(),
            "data-hovered": hovered,
            style: "--rule-tone: {tone.css()}",
            div { class: "curation-rule-line",
                // The mark is the rule's handle: touch it to light the tiles
                // it struck, press it to pause the rule. Editing the words
                // beside it leaves the sheet alone.
                button {
                    r#type: "button", class: "curation-rule-mark",
                    "aria-pressed": rule.enabled(),
                    "aria-label": "Rule active",
                    title: if rule.enabled() { "Active · hover to light its episodes · click to pause" } else { "Paused · click to activate" },
                    onmouseenter: handle.enter(), onmouseleave: handle.leave(),
                    onfocus: handle.enter(), onblur: handle.leave(),
                    onclick: move |_| {
                        let id = toggle_id.clone();
                        controller.edit(move |spec| if let Some(rule) = spec.rule_mut(&id) { let enabled = !rule.enabled(); rule.set_enabled(enabled); });
                    },
                    span { class: "curation-rule-dot", "aria-hidden": "true" }
                }
                span { class: "curation-rule-word", "If" }
                ConditionEditor {
                    catalog: catalog.clone(),
                    condition: rule.condition().clone(),
                    on_change: move |condition: RuleCondition| {
                        let id = condition_id.clone();
                        controller.edit(move |spec| if let Some(rule) = spec.rule_mut(&id) { *rule.condition_mut() = condition; });
                    },
                }
            }
            div { class: "curation-rule-line curation-rule-consequence",
                span { class: "curation-rule-word", "exclude as" }
                input {
                    class: "curation-input curation-rule-label",
                    "aria-label": "Reason shown on excluded episodes",
                    title: "Left empty, the excluded tile prints the rule itself",
                    placeholder: "the rule's own words",
                    value: "{rule.label()}",
                    oninput: move |event| {
                        let id = label_id.clone();
                        let label = event.value();
                        controller.edit(move |spec| if let Some(rule) = spec.rule_mut(&id) { rule.set_label(label); });
                    },
                }
                if let Some(removed) = removed {
                    span { class: "curation-rule-count", title: "Episodes this rule excluded in the visible run · hover to light them",
                        onmouseenter: handle.enter(), onmouseleave: handle.leave(),
                        strong { "{removed}" } " removed"
                    }
                }
                Action {
                    purpose: ActionPurpose::Remove, variant: ActionVariant::Icon,
                    aria_label: "Remove rule", title: "Remove rule",
                    onclick: move |_| { let id = remove_id.clone(); controller.edit(move |spec| spec.remove_rule(&id)); },
                    CloseIcon {}
                }
            }
            if let Some(problem) = problem {
                p { class: "curation-rule-invalid", role: "status", "This rule {problem}." }
            }
        }
    }
}

/// The rule's hover handle: entering lights the tiles the rule struck,
/// leaving settles the sheet.
#[derive(Clone, Copy)]
struct RuleHandle {
    controller: CurationController,
    id: Signal<String>,
}

impl RuleHandle {
    fn new(controller: CurationController, id: String) -> Self {
        Self {
            controller,
            id: use_signal(|| id),
        }
    }
    fn enter<E>(self) -> impl FnMut(E) + 'static {
        move |_| {
            let id = self.id.peek().clone();
            self.controller.set_hovered_rule(Some(id));
        }
    }
    fn leave<E>(self) -> impl FnMut(E) + 'static {
        move |_| self.controller.set_hovered_rule(None)
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

/// What the kept episodes receive, all of them or only those a condition picks.
#[component]
fn TransformsSection(controller: CurationController) -> Element {
    let spec = controller.spec();
    let catalog = spec.catalog();
    let outcome = controller.outcome();
    let judged = controller.judged_spec();
    let counts_current = judged
        .as_ref()
        .is_some_and(|judged| spec.accepts_output_from(judged));
    rsx! {
        div { class: "curation-section",
            div { class: "curation-section-title",
                span { "Transforms" }
                span { class: "curation-section-count", "{spec.transforms().len()}" }
            }
            if spec.transforms().is_empty() {
                p { class: "curation-section-note",
                    "Recorded for the kept episodes; Studio does not materialize them yet."
                }
            }
            for transform in spec.transforms().iter().cloned() {
                TransformEditor {
                    key: "{transform.id()}",
                    controller,
                    transform: transform.clone(),
                    catalog: catalog.clone(),
                    reached: counts_current.then(|| outcome.as_ref().map(|outcome| outcome.transform_count(transform.id()))).flatten(),
                }
            }
            label { r#for: "curation-add-transform", class: "sr-only", "Add a transform" }
            select {
                id: "curation-add-transform", class: "curation-select curation-add",
                value: "",
                onchange: move |event| {
                    let kind = event.value();
                    controller.edit(|spec| NewTransform::add(spec, &kind));
                },
                option { value: "", disabled: true, selected: true, "Add a transform…" }
                option { value: "camera_color_profile", "Camera colour profile — convert frames with one colour assumption" }
                option { value: "resize_camera", "Resize camera — scale frames to a fixed size" }
                option { value: "trim_time", "Trim time — keep a window of episode time" }
            }
        }
    }
}

struct NewTransform;

impl NewTransform {
    fn add(spec: &mut CurationSpec, kind: &str) {
        let operation = match kind {
            "camera_color_profile" => TransformOperation::CameraColorProfile {
                topic: TopicRef::new(""),
                profile: TransformOperation::COLOR_PROFILES[1].to_owned(),
            },
            "resize_camera" => TransformOperation::ResizeCamera {
                topic: TopicRef::new(""),
                width: 224,
                height: 224,
            },
            "trim_time" => TransformOperation::TrimTime {
                start_seconds: 0.0,
                end_seconds: None,
            },
            _ => return,
        };
        let id = format!(
            "transform-{}-{:x}",
            js_sys::Date::now() as u64,
            (js_sys::Math::random() * u32::MAX as f64) as u64
        );
        spec.add_transform(CurationTransform::new(id, "", operation));
    }
}

#[component]
fn TransformEditor(
    controller: CurationController,
    transform: CurationTransform,
    catalog: FactCatalog,
    reached: Option<u64>,
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
    let remove_id = id.clone();
    let label_id = id.clone();
    let summary = transform.operation().summary();
    rsx! {
        div { class: "curation-fact curation-transform", "data-expanded": expanded(),
            div { class: "curation-fact-summary",
                Action {
                    purpose: ActionPurpose::Toggle, variant: ActionVariant::Quiet, class: "curation-fact-disclosure",
                    aria_expanded: expanded(), title: "{transform.operation().kind_label()} · {summary}",
                    onclick: move |_| expanded.toggle(),
                    ChevronIcon { open: expanded() }
                    span { class: "curation-fact-summary-label", "{transform.title()}" }
                    if let Some(reached) = reached {
                        span { class: "curation-rule-count", title: "Kept episodes this transform would reach",
                            strong { "{reached}" } " reached"
                        }
                    }
                }
                Action {
                    purpose: ActionPurpose::Remove, variant: ActionVariant::Icon,
                    aria_label: "Remove transform", title: "Remove transform",
                    onclick: move |_| { let id = remove_id.clone(); controller.edit(move |spec| spec.remove_transform(&id)); },
                    CloseIcon {}
                }
            }
            if expanded() {
                input {
                    class: "curation-input curation-fact-label",
                    "aria-label": "Transform label",
                    placeholder: "{summary}",
                    value: "{transform.label()}",
                    oninput: move |event| {
                        let id = label_id.clone();
                        let label = event.value();
                        controller.edit(move |spec| if let Some(transform) = spec.transform_mut(&id) { transform.set_label(label); });
                    },
                }
                div { class: "curation-field-grid",
                    match transform.operation().clone() {
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
                div { class: "curation-rule-line",
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
            if let Some(problem) = problem {
                p { class: "curation-rule-invalid", role: "status", "This transform {problem}." }
            }
        }
    }
}
