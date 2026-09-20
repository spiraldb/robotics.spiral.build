//! Shared query result table with media previews and exact row inspection.
//!
//! One table serves Data and Workflow. Cells are painted by kind (see
//! `cells`), rows enter with a short stagger, and the inspector beside the
//! table walks rows with the arrow keys so a hundred frames can be reviewed
//! without leaving the keyboard.

use super::cells::ResultColumn;
use super::*;
use spiral_robotics_ui::{SheetHeading, SheetPosition, SideSheet, Toggle};

const STAGGERED_ROWS: usize = 24;

struct QueryCellContext<'a> {
    column: &'a QueryColumn,
    row: &'a serde_json::Map<String, serde_json::Value>,
    row_index: usize,
    row_count: usize,
}

impl<'a> QueryCellContext<'a> {
    const fn new(
        column: &'a QueryColumn,
        row: &'a serde_json::Map<String, serde_json::Value>,
        position: SheetPosition,
    ) -> Self {
        Self {
            column,
            row,
            row_index: position.index(),
            row_count: position.count(),
        }
    }

    fn image_alt(&self) -> String {
        let prefix = self
            .column
            .name()
            .strip_suffix("_preview")
            .or_else(|| self.column.name().strip_suffix("_image"));
        let mut context = Vec::new();
        for (label, field) in [
            ("episode", "episode_id"),
            ("stream", "stream_id"),
            ("frame", "frame_index"),
        ] {
            let specific = prefix.map(|prefix| format!("{prefix}_{field}"));
            let value = specific
                .as_deref()
                .and_then(|field| self.row.get(field))
                .or_else(|| self.row.get(field))
                .map(QueryJsonValue::cell_value)
                .filter(|value| value != "null");
            if let Some(value) = value {
                context.push(format!("{label} {value}"));
            }
        }
        let context = if context.is_empty() {
            String::new()
        } else {
            format!(" for {}", context.join(", "))
        };
        format!(
            "{}{}; result {} of {}",
            self.column.humanized_name(),
            context,
            self.row_index + 1,
            self.row_count
        )
    }
}

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
    let mut columns = result
        .columns()
        .iter()
        .map(|column| ResultColumn::analyze(column, result.rows()))
        .collect::<Vec<_>>();
    columns.sort_by_key(ResultColumn::priority);
    let row_count = result.rows().len();
    let column_count = columns.len();
    rsx! {
        div { class: "query-result",
            div { class: "query-result-tools",
                span { class: "query-result-shape",
                    if column_count == 1 { "1 column" } else { "{column_count} columns" }
                }
                Toggle { label: "Types".to_owned(), checked: show_types(), onchange: move |checked| show_types.set(checked) }
            }
            div { class: "query-result-body",
                div { class: "query-result-table-scroll",
                    table { class: "query-table", "aria-label": "Query results", "aria-rowcount": "{row_count}",
                        thead { tr {
                            th { scope: "col", class: "query-row-number", span { class: "sr-only", "Row" } }
                            for column in &columns {
                                th { key: "{column.column().name()}", scope: "col",
                                    class: column.cell_class(None),
                                    title: format!("{} · {}", column.column().name(), column.column().presentation_title()),
                                    span { class: "query-column-name", "{column.label()}" }
                                    if show_types() { span { class: "query-column-type", "{column.kind_label()}" } }
                                }
                            }
                        } }
                        tbody {
                            for (index, row) in result.rows().iter().enumerate() {
                                tr { key: "{index}",
                                    style: "--row: {index.min(STAGGERED_ROWS)}",
                                    "data-selected": selected_row == Some(index),
                                    "aria-selected": selected_row == Some(index),
                                    td { class: "query-row-number",
                                        Button { purpose: ActionPurpose::Select, variant: ButtonVariant::Link,
                                            class: "query-row-action",
                                            aria_label: "Inspect row {index + 1}", aria_pressed: selected_row == Some(index),
                                            id: "{prefix}-row-{index}",
                                            onclick: { let trigger = format!("{prefix}-row-{index}"); let inspector_id = inspector_id.clone(); move |_| {
                                                selected.set(Some(RowInspection::new(index, trigger.clone())));
                                                BrowserFocus::after_render(inspector_id.clone());
                                            } }, "{index + 1}" }
                                    }
                                    for (column_index, column) in columns.iter().enumerate() {
                                        { let value = row.get(column.column().name());
                                          if column.is_image() {
                                            rsx! { td { class: "query-media-cell",
                                                Button { purpose: ActionPurpose::Select, variant: ButtonVariant::Quiet,
                                                    class: "query-result-image-action", aria_label: "Inspect {column.column().humanized_name()} in row {index + 1}",
                                                    id: "{prefix}-image-{index}-{column_index}",
                                                    onclick: { let trigger = format!("{prefix}-image-{index}-{column_index}"); let inspector_id = inspector_id.clone(); move |_| {
                                                        selected.set(Some(RowInspection::new(index, trigger.clone())));
                                                        BrowserFocus::after_render(inspector_id.clone());
                                                    } },
                                                    if let Some(src) = column.column().image_src(value) {
                                                        img { class: "query-image-preview", src, alt: QueryCellContext::new(column.column(), row, SheetPosition::new(index, row_count)).image_alt(), loading: "lazy", decoding: "async", draggable: "false" }
                                                    } else { span { class: "query-image-unavailable", "No image" } }
                                                }
                                            } }
                                          } else {
                                            let trigger = format!("{prefix}-row-{index}");
                                            let inspector_id = inspector_id.clone();
                                            let bar = column.measure_bar(value);
                                            let title = if bar.is_some() { column.measure_title(value, row_count) } else { column.exact(value) };
                                            rsx! { td { class: column.cell_class(value), title,
                                                onclick: move |_| {
                                                    selected.set(Some(RowInspection::new(index, trigger.clone())));
                                                    BrowserFocus::after_render(inspector_id.clone());
                                                },
                                                if let Some(hue) = column.category_hue(value) {
                                                    span { class: "query-chip", style: "--hue: var(--tone-category-{hue})", "{column.display(value)}" }
                                                } else if let Some(bar) = bar {
                                                    span { class: "query-measure",
                                                        class: if bar.negative { "is-negative" },
                                                        class: if bar.zero.is_some() { "is-signed" },
                                                        style: "--start: {bar.start:.4}; --end: {bar.end:.4}; --zero: {bar.zero.unwrap_or(0.0):.4}",
                                                        span { class: "query-measure-text", "{column.display(value)}" }
                                                        span { class: "query-measure-bar", "aria-hidden": "true" }
                                                    }
                                                } else {
                                                    "{column.display(value)}"
                                                }
                                            } }
                                          }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(index) = selected_row.filter(|index| *index < row_count) {
                    QueryRowInspector {
                        key: "{index}",
                        row: result.rows()[index].clone(),
                        columns: columns.clone(),
                        position: SheetPosition::new(index, row_count),
                        ids: InspectorIds { inspector: inspector_id.clone(), prefix: prefix.clone() },
                        selected,
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct InspectorIds {
    inspector: String,
    prefix: String,
}

#[component]
fn QueryRowInspector(
    row: serde_json::Map<String, serde_json::Value>,
    columns: Vec<ResultColumn>,
    position: SheetPosition,
    ids: InspectorIds,
    selected: Signal<Option<RowInspection>>,
) -> Element {
    let heading = SheetHeading::new(
        ids.inspector.clone(),
        format!("Row {}", position.index() + 1),
        "row",
    )
    .with_detail(format!("of {}", position.count()));
    rsx! {
        SideSheet {
            heading,
            position,
            on_step: move |delta| if let Some(next) = position.stepped(delta) { RowInspection::step(selected, next, &ids) },
            on_close: move |_| RowInspection::close(selected),
            div { class: "query-row-inspector-media",
                for column in columns.iter().filter(|column| column.is_image()) {
                    figure { key: "{column.column().name()}",
                        if let Some(src) = column.column().image_src(row.get(column.column().name())) {
                            img { src, alt: column.column().humanized_name() }
                        } else {
                            span { class: "query-image-unavailable", "No image" }
                        }
                        figcaption { "{column.label()}" }
                    }
                }
            }
            dl { class: "query-row-inspector-facts",
                for column in columns.iter().filter(|column| !column.is_image()) {
                    div { key: "{column.column().name()}",
                        dt { title: column.column().presentation_title(), code { "{column.column().name()}" } }
                        dd {
                            { let value = row.get(column.column().name());
                              rsx! {
                                if let Some(hue) = column.category_hue(value) {
                                    span { class: "query-chip", style: "--hue: var(--tone-category-{hue})", "{column.display(value)}" }
                                } else {
                                    "{column.exact(value)}"
                                }
                              } }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
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

    /// Move the inspection to a neighbouring row. Focus returns to the new
    /// row's own action when the inspector closes.
    fn step(mut selected: Signal<Option<Self>>, next: SheetPosition, ids: &InspectorIds) {
        let index = next.index();
        selected.set(Some(Self::new(
            index,
            format!("{}-row-{index}", ids.prefix),
        )));
        BrowserFocus::after_render(ids.inspector.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn inline_image_alt_uses_camera_specific_identity_and_row_position() {
        let column = QueryColumn::new("wrist_preview", "Utf8", false)
            .with_semantic_type(Some(IMAGE_EXTENSION_NAME.to_owned()));
        let row = serde_json::Map::from_iter([
            ("episode_id".to_owned(), serde_json::json!("take-12")),
            (
                "wrist_stream_id".to_owned(),
                serde_json::json!("follower/wrist"),
            ),
            ("wrist_frame_index".to_owned(), serde_json::json!(42)),
        ]);
        assert_eq!(
            QueryCellContext::new(&column, &row, SheetPosition::new(2, 18)).image_alt(),
            "Wrist for episode take-12, stream follower/wrist, frame 42; result 3 of 18"
        );
    }
}
