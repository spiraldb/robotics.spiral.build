//! Shared query result table with media previews and exact row inspection.

use super::*;

#[component]
pub(super) fn QueryResultTable(result: SpiralQuerySnapshot) -> Element {
    let prefix = use_hook(|| format!("query-result-{}", dioxus::dioxus_core::current_scope_id().0));
    let inspector_id = format!("{prefix}-inspector");
    let mut selected = use_signal(|| None::<RowInspection>);
    use_effect(use_reactive((&result,), move |_| selected.set(None)));
    let selected_row = selected
        .read()
        .as_ref()
        .map(|inspection| inspection.row_index);
    let mut show_types = use_signal(|| false);
    let mut columns = result.columns().iter().collect::<Vec<_>>();
    columns.sort_by_key(|column| ResultColumn(column).priority());
    rsx! {
        div { class: "query-result",
            div { class: "query-result-options",
                span { "Select a row to inspect exact values" }
                label { input { r#type: "checkbox", checked: show_types(), onchange: move |event| show_types.set(event.checked()) } "Column types" }
            }
            div { class: "query-result-body",
                div { class: "query-result-table-scroll",
                    table { class: "query-table query-result-table", "aria-label": "Query results",
                        thead { tr {
                            th { scope: "col", class: "query-row-number", "Row" }
                            for column in &columns {
                                th { key: "{column.name()}", scope: "col", title: format!("{} · {}", column.name(), column.presentation_title()),
                                    span { class: "query-column-name", "{ResultColumn(column).label()}" }
                                    if show_types() { span { class: "query-column-type", "{column.presentation_kind()}" } }
                                }
                            }
                        } }
                        tbody {
                            for (index, row) in result.rows().iter().enumerate() {
                                tr { key: "{index}", "data-selected": selected_row == Some(index),
                                    td { class: "query-row-number",
                                        Action { purpose: ActionPurpose::Select, variant: ActionVariant::Link,
                                            aria_label: "Inspect row {index + 1}", aria_pressed: selected_row == Some(index),
                                            id: "{prefix}-row-{index}",
                                            onclick: { let trigger = format!("{prefix}-row-{index}"); let inspector_id = inspector_id.clone(); move |_| {
                                                selected.set(Some(RowInspection::new(index, trigger.clone())));
                                                BrowserFocus::after_render(inspector_id.clone());
                                            } }, "{index + 1}" }
                                    }
                                    for (column_index, column) in columns.iter().enumerate() {
                                        if column.is_image() {
                                            td { class: "query-media-cell",
                                                Action { purpose: ActionPurpose::Select, variant: ActionVariant::Quiet,
                                                    class: "query-result-image-action", aria_label: "Inspect {column.humanized_name()} in row {index + 1}",
                                                    id: "{prefix}-image-{index}-{column_index}",
                                                    onclick: { let trigger = format!("{prefix}-image-{index}-{column_index}"); let inspector_id = inspector_id.clone(); move |_| {
                                                        selected.set(Some(RowInspection::new(index, trigger.clone())));
                                                        BrowserFocus::after_render(inspector_id.clone());
                                                    } },
                                                    if let Some(src) = column.image_src(row.get(column.name())) {
                                                        img { class: "query-image-preview", src, alt: QueryCellContext::new(column, row, QueryRowPosition::new(index, result.rows().len())).image_alt(), loading: "lazy", decoding: "async", draggable: "false" }
                                                    } else { span { class: "query-image-unavailable", "No image" } }
                                                }
                                            }
                                        } else {
                                            td { class: row.get(column.name()).presentation_class(), title: row.get(column.name()).cell_text(),
                                                "{ResultValue(row.get(column.name())).display()}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(index) = selected_row.filter(|index| *index < result.rows().len()) {
                    aside { class: "query-row-inspector", "aria-label": "Row {index + 1} details",
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Escape {
                                event.prevent_default();
                                RowInspection::close(selected);
                            }
                        },
                        header {
                            h3 { id: "{inspector_id}", tabindex: "-1", "Row {index + 1}" }
                            Action { purpose: ActionPurpose::Close, variant: ActionVariant::Link,
                                onclick: move |_| RowInspection::close(selected), "Close" }
                        }
                        dl {
                            for column in &columns {
                                div {
                                    dt { title: column.presentation_title(), "{column.name()}" }
                                    dd {
                                        if column.is_image() {
                                            if let Some(src) = column.image_src(result.rows()[index].get(column.name())) {
                                                img { src, alt: column.humanized_name() }
                                            } else { "No image" }
                                        } else {
                                            {result.rows()[index].get(column.name()).map(QueryJsonValue::cell_value).unwrap_or_else(|| "null".to_owned())}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct RowInspection {
    row_index: usize,
    return_focus: String,
}

impl RowInspection {
    fn close(mut selected: Signal<Option<Self>>) {
        let inspection = selected.write().take();
        if let Some(inspection) = inspection {
            BrowserFocus::after_render(inspection.return_focus);
        }
    }

    fn new(row_index: usize, return_focus: String) -> Self {
        Self {
            row_index,
            return_focus,
        }
    }
}

struct ResultColumn<'a>(&'a QueryColumn);

impl ResultColumn<'_> {
    fn priority(&self) -> u8 {
        let name = self.0.name();
        if self.0.is_image() {
            0
        } else if name.ends_with("decision") || name.ends_with("status") || name.ends_with("label")
        {
            1
        } else if name.ends_with("_id") || name.ends_with("_path") {
            3
        } else {
            2
        }
    }

    fn label(&self) -> String {
        let mut label = self.0.name().replace('_', " ");
        if let Some(first) = label.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        label
    }
}

struct ResultValue<'a>(Option<&'a serde_json::Value>);

impl ResultValue<'_> {
    fn display(&self) -> String {
        match self.0 {
            None | Some(serde_json::Value::Null) => "—".to_owned(),
            Some(serde_json::Value::Number(number)) if number.is_f64() => {
                let value = number.as_f64().unwrap_or_default();
                if value == 0.0 {
                    return "0".to_owned();
                }
                if value.abs() < 0.00001 || value.abs() >= 1e9 {
                    return format!("{value:.4e}");
                }
                let decimals = (5.0 - value.abs().log10().floor()).clamp(0.0, 10.0) as usize;
                let text = format!("{value:.decimals$}");
                if text.contains('.') {
                    text.trim_end_matches('0').trim_end_matches('.').to_owned()
                } else {
                    text
                }
            }
            value => value.cell_text(),
        }
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn readable_floats_preserve_integer_identity_and_nonzero_magnitudes() {
        assert_eq!(
            ResultValue(Some(&serde_json::json!(18446744073709551615_u64))).display(),
            "18446744073709551615"
        );
        assert_eq!(
            ResultValue(Some(&serde_json::json!(0.002061118))).display(),
            "0.00206112"
        );
        assert_eq!(
            ResultValue(Some(&serde_json::json!(0.000000002))).display(),
            "2.0000e-9"
        );
        assert_eq!(
            ResultValue(Some(&serde_json::json!(0.03))).display(),
            "0.03"
        );
        assert_eq!(ResultValue(None).display(), "—");
    }
}
