//! Natural-language and manual authoring share one editable, validated proposal.
use super::{client::CurationClient, state::CurationController};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;
use spiral_robotics_common::curation::{
    facts::TopicRef,
    interpretation::{InterpretationRequest, RuleIntent, RuleProposal},
    rules::{ConditionOp, CurationTransform, TransformOperation},
};

#[component]
pub(super) fn RuleComposer(controller: CurationController, camera_topics: Vec<String>) -> Element {
    let mut manual_operation = use_signal(|| "duration".to_owned());
    let mut prompt = use_signal(String::new);
    let mut proposal = use_signal(|| None::<RuleProposal>);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let can_add = !pending() && !prompt().trim().is_empty();
    rsx! {
        div { class: "curation-section curation-composer",
            label { r#for: "curation-rule-description", class: "curation-section-title", "Add a rule" }
            textarea { id: "curation-rule-description", class: "curation-input curation-rule-prompt", rows: "3",
                placeholder: "Exclude episodes shorter than 5 seconds…", value: "{prompt}", disabled: pending(),
                oninput: move |event| { prompt.set(event.value()); proposal.set(None); error.set(None); } }
            Action { purpose: ActionPurpose::Create, variant: ActionVariant::Solid, disabled: !can_add,
                onclick: { let topics = camera_topics.clone(); move |_| {
                    if pending() { return; }
                    pending.set(true); error.set(None); proposal.set(None);
                    let request = InterpretationRequest::new(prompt(), topics.clone());
                    let original = controller.spec();
                    spawn(async move {
                        let result = CurationClient::current().interpret(&request).await;
                        pending.set(false);
                        let current = controller.spec();
                        if current.id() != original.id() || current.source() != original.source() {
                            error.set(Some("The recipe or dataset changed. Click Add rule again for the current recipe.".into()));
                            return;
                        }
                        match result {
                            Ok(draft) if draft.problems().is_empty() => match controller.add_proposal(&draft) {
                                Ok(()) => prompt.set(String::new()),
                                Err(message) => error.set(Some(message)),
                            },
                            Ok(draft) => proposal.set(Some(draft)),
                            Err(problem) => error.set(Some(problem.to_string())),
                        }
                    });
                } },
                if pending() { "Adding…" } else { "Add rule" }
            }
            if let Some(error) = error() { p { class: "curation-rule-invalid", role: "alert", "{error}" } }
            if let Some(draft) = proposal() {
                ProposalEditor { proposal: draft.clone(), cameras: camera_topics.clone(), on_change: move |next| proposal.set(Some(next)) }
                for note in draft.notes() { p { class: "curation-section-note", "{note}" } }
                for problem in draft.problems() { p { class: "curation-rule-invalid", "{problem}" } }
                Action { purpose: ActionPurpose::Create, variant: ActionVariant::Solid, disabled: pending() || !draft.problems().is_empty(),
                    onclick: move |_| {
                        if pending() { return; }
                        match controller.add_proposal(&draft) {
                            Ok(()) => { proposal.set(None); prompt.set(String::new()); }
                            Err(message) => error.set(Some(message)),
                        }
                    }, "Add rule" }
            }
            details { class: "curation-manual-builder",
                summary { "Build manually" }
                label { r#for: "curation-manual-operation", class: "curation-section-note", "Operation" }
                select { id: "curation-manual-operation", class: "curation-select", value: manual_operation(),
                    onchange: move |event| manual_operation.set(event.value()),
                    for intent in RuleIntent::ALL { option { value: intent.key(), "{intent.label()}" } }
                    option { value: "resize_camera", "Resize camera" }
                    option { value: "camera_color_profile", "Camera colour profile" }
                    option { value: "trim_time", "Trim time" }
                }
                Action { purpose: ActionPurpose::Create, variant: ActionVariant::Quiet,
                    onclick: { let cameras = camera_topics.clone(); move |_| {
                        error.set(None);
                        let operation = ManualTransform::operation(&manual_operation(), cameras.first());
                        if let Some(operation) = operation {
                            let id = format!("transform-{}-{:x}", js_sys::Date::now() as u64, (js_sys::Math::random() * u32::MAX as f64) as u64);
                            controller.edit(|spec| spec.add_transform(CurationTransform::new(id.clone(), "", operation)));
                            controller.select_pipeline(super::pipeline::PipelineSelection::Step(id));
                            proposal.set(None);
                        } else {
                            let mut draft = RuleProposal::default();
                            draft.set_intent(RuleIntent::from_key(&manual_operation()));
                            if draft.problems().is_empty() {
                                match controller.add_proposal(&draft) {
                                    Ok(()) => proposal.set(None),
                                    Err(message) => error.set(Some(message)),
                                }
                            } else {
                                proposal.set(Some(draft));
                            }
                        }
                    } }, "Add rule" }

            }
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
        div { class: "curation-proposal", "aria-label": "Interpreted rule",
            label { "Operation"
                select { class: "curation-select", value: proposal.intent().key(),
                    onchange: { let proposal = proposal.clone(); move |event| { let mut next = proposal.clone(); next.set_intent(RuleIntent::from_key(&event.value())); on_change.call(next); } },
                    for intent in RuleIntent::ALL { option { value: intent.key(), selected: intent == proposal.intent(), "{intent.label()}" } }
                }
            }
            if matches!(proposal.intent(), RuleIntent::CameraLuminance) {
                label { "Camera"
                    select { class: "curation-select", value: proposal.camera().unwrap_or_default(),
                        onchange: { let proposal = proposal.clone(); move |event| { let mut next = proposal.clone(); next.set_camera(Some(event.value())); on_change.call(next); } },
                        option { value: "", "Choose camera" }
                        for camera in cameras { option { value: "{camera}", selected: proposal.camera() == Some(camera.as_str()), "{camera}" } }
                    }
                }
            }
            if !matches!(proposal.intent(), RuleIntent::ProductiveChunks) {
            label { "Exclude episodes when"
                select { class: "curation-select", value: proposal.comparison().map(ConditionOp::key).unwrap_or_default(),
                    onchange: { let proposal = proposal.clone(); move |event| { let mut next = proposal.clone(); next.set_comparison(ConditionOp::from_key(&event.value())); on_change.call(next); } },
                    option { value: "", "Choose comparison" }
                    for op in [ConditionOp::Below, ConditionOp::AtMost, ConditionOp::Above, ConditionOp::AtLeast] {
                        option { value: op.key(), selected: proposal.comparison() == Some(op), "{op.label()}" }
                    }
                }
            }
            label {
                if proposal.intent() == RuleIntent::Duration { "Threshold (seconds)" } else { "Threshold" }
                input { class: "curation-input", inputmode: "decimal", value: "{number}", placeholder: "Enter threshold",
                    oninput: { let proposal = proposal.clone(); move |event| {
                        let raw = event.value(); number.set(raw.clone()); let mut next = proposal.clone(); next.set_value(raw.parse::<f64>().ok());
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
