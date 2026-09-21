//! One place to add the next step: describe it for the browser-owned interpreter,
//! or pick a kind and fill in the row that appears.
use super::{pipeline::PipelineSelection, state::CurationController};
use crate::client::model::ModelClient;
use dioxus::prelude::*;
use spiral_robotics_common::curation::{
    facts::TopicRef,
    interpretation::{InterpretationRequest, RuleIntent, RuleProposal},
    rules::{
        ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition,
        TransformOperation,
    },
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Field, Select, TextInput, TextInputKind,
};

#[component]
pub(super) fn RuleComposer(controller: CurationController, camera_topics: Vec<String>) -> Element {
    let model = use_hook(ModelClient::current);
    let mut prompt = use_signal(String::new);
    let mut proposal = use_signal(|| None::<RuleProposal>);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    // The kind picker remounts after every pick so it always reads its prompt.
    let mut picks = use_signal(|| 0_u32);
    let can_add = !pending() && !prompt().trim().is_empty();
    let interpret = {
        let topics = camera_topics.clone();
        let model = model.clone();
        move |_| {
            if pending() {
                return;
            }
            pending.set(true);
            error.set(None);
            proposal.set(None);
            let request = InterpretationRequest::new(prompt(), topics.clone());
            let original = controller.spec();
            let model = model.clone();
            spawn(async move {
                let result =
                    super::interpretation::RuleInterpreter::interpret(model, request).await;
                pending.set(false);
                let current = controller.spec();
                if current.id() != original.id() || current.source() != original.source() {
                    error.set(Some(
                        "The recipe or dataset changed. Add the step again for the current recipe."
                            .into(),
                    ));
                    return;
                }
                match result {
                    Ok(draft) if draft.problems().is_empty() => {
                        match controller.add_proposal(&draft) {
                            Ok(()) => prompt.set(String::new()),
                            Err(message) => error.set(Some(message)),
                        }
                    }
                    Ok(draft) => proposal.set(Some(draft)),
                    Err(problem) => error.set(Some(problem.to_string())),
                }
            });
        }
    };
    rsx! {
        section { class: "curation-section curation-composer", "aria-label": "Add a step",
            div { class: "curation-section-title", span { "Add a step" } }
            div { class: "curation-composer-box",
                label { r#for: "curation-rule-description", class: "sr-only", "Describe a step" }
                textarea { id: "curation-rule-description", class: "curation-composer-input", rows: "2",
                    placeholder: "Describe it, e.g. exclude episodes shorter than 5 seconds",
                    value: "{prompt}", disabled: pending(),
                    oninput: move |event| { prompt.set(event.value()); proposal.set(None); error.set(None); } }
                div { class: "curation-composer-actions",
                    label { r#for: "curation-step-kind", class: "sr-only", "Add a step by kind" }
                    Select { key: "picker-{picks}", id: "curation-step-kind", class: "curation-step-picker", value: "",
                        title: "Add a step of a chosen kind and fill it in",
                        onchange: {
                            let cameras = camera_topics.clone();
                            move |value: String| {
                                error.set(None);
                                match add_by_kind(controller, &value, cameras.first()) {
                                    Ok(Some(draft)) => proposal.set(Some(draft)),
                                    Ok(None) => proposal.set(None),
                                    Err(message) => error.set(Some(message)),
                                }
                                picks += 1;
                            }
                        },
                        option { value: "", disabled: true, selected: true, "Add by kind…" }
                        optgroup { label: "Filter",
                            option { value: "duration", "Episode duration" }
                            option { value: "message_count", "Message count" }
                            option { value: "camera_luminance", "Camera brightness" }
                        }
                        optgroup { label: "Transform",
                            option { value: "productive_chunks", "Remove unproductive moments" }
                            option { value: "resize_camera", "Resize camera" }
                            option { value: "camera_color_profile", "Camera colour profile" }
                            option { value: "trim_time", "Trim time" }
                        }
                    }
                    Button { purpose: ActionPurpose::Create, variant: ButtonVariant::Solid, disabled: !can_add,
                        aria_busy: pending(), title: "Interpret the description as a step",
                        onclick: interpret,
                        if pending() { "Adding…" } else { "Add" }
                    }
                }
            }
            if let Some(error) = error() { p { class: "curation-rule-invalid", role: "alert", "{error}" } }
            if let Some(draft) = proposal() {
                ProposalEditor { proposal: draft.clone(), cameras: camera_topics.clone(), on_change: move |next| proposal.set(Some(next)) }
                for note in draft.notes() { p { class: "curation-section-note", "{note}" } }
                for problem in draft.problems() { p { class: "curation-rule-invalid", "{problem}" } }
                Button { purpose: ActionPurpose::Create, variant: ButtonVariant::Solid, disabled: pending() || !draft.problems().is_empty(),
                    onclick: move |_| {
                        if pending() { return; }
                        match controller.add_proposal(&draft) {
                            Ok(()) => { proposal.set(None); prompt.set(String::new()); }
                            Err(message) => error.set(Some(message)),
                        }
                    }, "Add step" }
            }
        }
    }
}

fn fresh_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{:x}",
        js_sys::Date::now() as u64,
        (js_sys::Math::random() * u32::MAX as f64) as u64
    )
}

/// Add a step of one kind. Filters over built-in facts and camera transforms
/// land in the recipe at once, ready to fill in; a kind that still needs a
/// choice (which camera to measure) comes back as a draft to complete.
fn add_by_kind(
    controller: CurationController,
    key: &str,
    camera: Option<&String>,
) -> Result<Option<RuleProposal>, String> {
    match key {
        "duration" | "message_count" => {
            let fact = if key == "duration" {
                "duration_s"
            } else {
                "messages"
            };
            let id = fresh_id("rule");
            let rule = CurationRule::new(
                id.clone(),
                "",
                RuleCondition::new(fact, ConditionOp::Below, ConditionValue::None),
            );
            controller.edit(move |spec| spec.add_rule(rule));
            controller.select_pipeline(PipelineSelection::Step(id));
            Ok(None)
        }
        "camera_luminance" | "productive_chunks" => {
            let mut draft = RuleProposal::default();
            draft.set_intent(RuleIntent::from_key(key));
            if draft.problems().is_empty() {
                controller.add_proposal(&draft).map(|()| None)
            } else {
                Ok(Some(draft))
            }
        }
        other => {
            let operation = ManualTransform::operation(other, camera)
                .ok_or_else(|| "That kind of step is not available.".to_owned())?;
            let id = fresh_id("transform");
            let transform = CurationTransform::new(id.clone(), "", operation);
            controller.edit(move |spec| spec.add_transform(transform));
            controller.select_pipeline(PipelineSelection::Step(id));
            Ok(None)
        }
    }
}

#[component]
fn ProposalEditor(
    proposal: RuleProposal,
    cameras: Vec<String>,
    on_change: EventHandler<RuleProposal>,
) -> Element {
    let mut number = use_signal(|| {
        proposal
            .value()
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    rsx! {
        div { class: "curation-proposal", "aria-label": "Interpreted step",
            Field { label: "Operation",
                Select { value: proposal.intent().key(),
                    onchange: { let proposal = proposal.clone(); move |value: String| { let mut next = proposal.clone(); next.set_intent(RuleIntent::from_key(&value)); on_change.call(next); } },
                    for intent in RuleIntent::ALL { option { value: intent.key(), selected: intent == proposal.intent(), "{intent.label()}" } }
                }
            }
            if matches!(proposal.intent(), RuleIntent::CameraLuminance) {
                Field { label: "Camera",
                    Select { value: proposal.camera().unwrap_or_default(),
                        onchange: { let proposal = proposal.clone(); move |value: String| { let mut next = proposal.clone(); next.set_camera(Some(value)); on_change.call(next); } },
                        option { value: "", "Choose camera" }
                        for camera in cameras { option { value: "{camera}", selected: proposal.camera() == Some(camera.as_str()), "{camera}" } }
                    }
                }
            }
            if !matches!(proposal.intent(), RuleIntent::ProductiveChunks) {
            Field { label: "Exclude episodes when",
                Select { value: proposal.comparison().map(ConditionOp::key).unwrap_or_default(),
                    onchange: { let proposal = proposal.clone(); move |value: String| { let mut next = proposal.clone(); next.set_comparison(ConditionOp::from_key(&value)); on_change.call(next); } },
                    option { value: "", "Choose comparison" }
                    for op in [ConditionOp::Below, ConditionOp::AtMost, ConditionOp::Above, ConditionOp::AtLeast] {
                        option { value: op.key(), selected: proposal.comparison() == Some(op), "{op.label()}" }
                    }
                }
            }
            Field {
                label: if proposal.intent() == RuleIntent::Duration { "Threshold (seconds)" } else { "Threshold" },
                TextInput { kind: TextInputKind::Decimal, value: "{number}", placeholder: "Enter threshold",
                    oninput: { let proposal = proposal.clone(); move |raw: String| {
                        number.set(raw.clone()); let mut next = proposal.clone(); next.set_value(raw.parse::<f64>().ok());
                        if next.intent() == RuleIntent::Duration { next.set_unit(Some("seconds".into())); }
                        on_change.call(next);
                    } }
                }
            }
            }
        }
    }
}

/// Manual-only operations reuse the canonical transforms and their ordered editors.
struct ManualTransform;

impl ManualTransform {
    fn operation(key: &str, camera: Option<&String>) -> Option<TransformOperation> {
        let topic = || TopicRef::new(camera.cloned().unwrap_or_default());
        match key {
            "resize_camera" => Some(TransformOperation::ResizeCamera {
                topic: topic(),
                width: 224,
                height: 224,
            }),
            "camera_color_profile" => Some(TransformOperation::CameraColorProfile {
                topic: topic(),
                profile: "bt709_full".into(),
            }),
            "trim_time" => Some(TransformOperation::TrimTime {
                start_seconds: 0.0,
                end_seconds: None,
            }),
            _ => None,
        }
    }
}
