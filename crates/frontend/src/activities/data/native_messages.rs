//! Message payloads lead; transport metadata and raw Arrow types are secondary.
use super::query_result::{QueryExecutionPresentation, QueryState};
use dioxus::prelude::*;
use serde_json::Value;

#[component]
pub(super) fn TopicMessagePreview(
    model: QueryExecutionPresentation,
    mut selected: Signal<usize>,
) -> Element {
    let mut raw_loaded = use_signal(|| false);
    let QueryState::Ready(result) = model.state() else {
        return model.render();
    };
    let rows = result.rows();
    if rows.is_empty() {
        return rsx! { p { class: "data-empty-copy", role: "status", "This topic returned no messages." } };
    }
    let index = selected().min(rows.len() - 1);
    let row = &rows[index];
    let payload = row.get("value").cloned();
    let metadata = row
        .iter()
        .filter(|(key, _)| key.as_str() != "value")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    rsx! {
        div { class: "data-message-summary", role: "status",
            span { "{rows.len()} messages in this preview" }
            if result.truncated() { span { "Preview limit reached" } }
        }
        div { class: "data-message-inspection",
            nav { class: "data-message-list", "aria-label": "Preview messages",
                for (row_index, row) in rows.iter().enumerate() {
                    button { key: "{row_index}", "aria-pressed": index == row_index, onclick: move |_| selected.set(row_index),
                        span { class: "block font-medium", "Message {row_index + 1}" }
                        if let Some(sequence) = row.get("sequence") { span { class: "mt-1 block text-[11px] text-ink-soft", "Sequence {sequence}" } }
                    }
                }
            }
            section { class: "data-message-fields", "aria-label": "Selected message",
                h4 { class: "mb-4 text-sm font-medium", "Message {index + 1}" }
                div { key: "message-{index}",
                    if let Some(Value::Object(fields)) = payload {
                        MessageFields { fields }
                    } else if let Some(value) = payload {
                        MessageValue { value }
                    } else {
                        p { class: "text-xs text-ink-soft", "No decoded value was returned. Open the raw table to inspect this message." }
                    }
                    details { class: "data-message-metadata",
                        summary { "Message metadata" }
                        MessageFields { fields: metadata }
                    }
                }
            }
        }
        details { class: "data-message-raw", ontoggle: move |_| { if !*raw_loaded.peek() { raw_loaded.set(true); } },
            summary { "Raw table and types" }
            if raw_loaded() { div { class: "data-message-raw-table", {model.render()} } }
        }
    }
}

#[component]
fn MessageFields(fields: serde_json::Map<String, Value>) -> Element {
    rsx! {
        dl { class: "data-message-field-list",
            for (name, value) in fields {
                div { key: "{name}", class: "data-message-field",
                    dt { "{name}" }
                    dd { MessageValue { value } }
                }
            }
        }
    }
}

#[component]
fn MessageValue(value: Value) -> Element {
    match value {
        Value::Null => rsx! { span { class: "text-ink-soft", "null" } },
        Value::Bool(value) => rsx! { span { "{value}" } },
        Value::Number(value) => rsx! { span { class: "tabular-nums", "{value}" } },
        Value::String(value) if value.chars().count() > 160 => rsx! {
            details { summary { "Text · {value.chars().count()} characters" }
                pre { class: "data-message-text", "{value}" }
            }
        },
        Value::String(value) if value.is_empty() => {
            rsx! { span { class: "text-ink-soft", "Empty string" } }
        }
        Value::String(value) => rsx! { span { class: "whitespace-pre-wrap", "{value}" } },
        Value::Array(values) => rsx! {
            details { summary { if values.len() == 1 { "1 item" } else { "{values.len()} items" } }
                ol { class: "data-message-array",
                    for (index, value) in values.into_iter().enumerate() {
                        li { key: "{index}", span { class: "text-ink-soft", "[{index}]" } MessageValue { value } }
                    }
                }
            }
        },
        Value::Object(fields) => rsx! {
            details { summary { if fields.len() == 1 { "1 field" } else { "{fields.len()} fields" } } MessageFields { fields } }
        },
    }
}
