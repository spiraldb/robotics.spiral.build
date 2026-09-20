//! Query execution snapshots and the shared result viewer.

use dioxus::prelude::*;
use spiral_robotics_common::relations::{QueryColumn, SpiralQuerySnapshot};
use spiral_robotics_ui::{ActionPurpose, BrowserFocus, Button, ButtonVariant};

mod cells;
mod table;

const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const VARIABLE_TENSOR_EXTENSION_NAME: &str = "arrow.variable_shape_tensor";
const ENCODED_MEDIA_EXTENSION_NAME: &str = "spql.media.EncodedMedia";

/// A complete, renderable snapshot of the Query tool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryExecutionPresentation {
    state: QueryState,
}

impl QueryExecutionPresentation {
    pub(in crate::activities) const fn new(state: QueryState) -> Self {
        Self { state }
    }

    const fn state(&self) -> &QueryState {
        &self.state
    }

    pub(in crate::activities) fn render(&self) -> Element {
        rsx! { QueryResultContent { model: self.clone() } }
    }
}

/// Query execution state carried to the visualization region.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueryState {
    Running,
    Ready(SpiralQuerySnapshot),
    Failed(String),
}

#[component]
fn QueryResultContent(model: QueryExecutionPresentation) -> Element {
    rsx! {
        match model.state() {
            QueryState::Running => rsx! { QueryResultSkeleton {} },
            QueryState::Failed(error) => rsx! {
                div { class: "m-3 rounded bg-danger-soft px-3 py-2 text-[11px] text-danger", role: "alert",
                    strong { class: "block font-medium", "Query failed" }
                    p { class: "mt-0.5", "{error}" }
                }
            },
            QueryState::Ready(result) if result.rows().is_empty() => rsx! {
                div { class: "flex flex-1 items-center justify-center p-6 text-center",
                    div {
                        strong { class: "block text-[12px] font-medium text-ink", "No rows" }
                        p { class: "mt-1 text-[11px] text-ink-faint", "The query completed successfully." }
                    }
                }
            },
            QueryState::Ready(result) => rsx! {
                table::QueryResultTable { result: result.clone() }
            },
        }
    }
}

/// Rows are on their way. The table's own geometry stands in for them, so the
/// panel does not collapse to a sentence and then jump when data lands.
#[component]
fn QueryResultSkeleton() -> Element {
    const COLUMN_WIDTHS: [u8; 6] = [5, 9, 4, 6, 7, 5];
    rsx! {
        div { class: "query-skeleton", role: "status", "aria-live": "polite", "aria-busy": "true",
            span { class: "sr-only", "Running query…" }
            div { class: "query-skeleton-row query-skeleton-head", "aria-hidden": "true",
                span { class: "query-skeleton-index" }
                span { class: "query-skeleton-media" }
                for (index, width) in COLUMN_WIDTHS.iter().enumerate() {
                    span { key: "{index}", class: "query-skeleton-cell", style: "--w: {width}" }
                }
            }
            for row in 0..7_u8 {
                div { key: "{row}", class: "query-skeleton-row", style: "--row: {row}", "aria-hidden": "true",
                    span { class: "query-skeleton-index" }
                    span { class: "query-skeleton-media" }
                    for (index, width) in COLUMN_WIDTHS.iter().enumerate() {
                        span { key: "{index}", class: "query-skeleton-cell", style: "--w: {width.saturating_sub((row + index as u8) % 3)}" }
                    }
                }
            }
        }
    }
}

trait QueryColumnPresentation {
    fn presentation_title(&self) -> String;
    fn humanized_name(&self) -> String;
    fn is_image(&self) -> bool;
    fn presentation_kind(&self) -> &str;
    fn image_src<'a>(&self, value: Option<&'a serde_json::Value>) -> Option<&'a str>;
}

impl QueryColumnPresentation for QueryColumn {
    fn presentation_title(&self) -> String {
        let mut title = self.data_type().to_owned();
        if let Some(semantic_type) = &self.semantic_type() {
            title.push_str(" · ");
            title.push_str(semantic_type);
        }
        if self.nullable() {
            title.push_str(" · nullable");
        }
        title
    }

    fn humanized_name(&self) -> String {
        let words = self
            .name()
            .trim_end_matches("_preview")
            .trim_end_matches("_image")
            .split('_')
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        let mut label = words.join(" ");
        if let Some(first) = label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        if label.is_empty() {
            "Image".to_owned()
        } else {
            label
        }
    }

    fn is_image(&self) -> bool {
        matches!(
            self.semantic_type(),
            Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME)
        )
    }

    fn presentation_kind(&self) -> &str {
        match self.semantic_type() {
            Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME) => "Image",
            Some(ENCODED_MEDIA_EXTENSION_NAME) => "Video",
            _ => self.data_type(),
        }
    }

    fn image_src<'a>(&self, value: Option<&'a serde_json::Value>) -> Option<&'a str> {
        self.image_data_url(value)
    }
}

trait QueryJsonValue {
    fn cell_value(&self) -> String;
}

impl QueryJsonValue for serde_json::Value {
    fn cell_value(&self) -> String {
        match self {
            serde_json::Value::String(value) => value.clone(),
            value => value.to_string(),
        }
    }
}

trait OptionalQueryValue {
    fn presentation_class(self) -> &'static str;
    fn cell_text(self) -> String;
}

impl OptionalQueryValue for Option<&serde_json::Value> {
    fn presentation_class(self) -> &'static str {
        match self {
            None | Some(serde_json::Value::Null) => "query-value-null",
            Some(serde_json::Value::Number(_)) => "query-value-number",
            Some(serde_json::Value::Bool(_)) => "query-value-boolean",
            _ => "",
        }
    }

    fn cell_text(self) -> String {
        let rendered = match self {
            None | Some(serde_json::Value::Null) => "null".to_owned(),
            Some(serde_json::Value::String(value)) => value.clone(),
            Some(value) => serde_json::to_string(value).unwrap_or_default(),
        };
        const LIMIT: usize = 240;
        if rendered.chars().count() > LIMIT {
            format!("{}…", rendered.chars().take(LIMIT).collect::<String>())
        } else {
            rendered
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_column(name: &str, semantic_type: Option<&str>) -> QueryColumn {
        QueryColumn::new(name, "Utf8", false).with_semantic_type(semantic_type.map(str::to_owned))
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn image_previews_require_both_semantic_type_and_an_encoded_image() {
        let value = serde_json::json!("data:image/jpeg;base64,preview");
        let image = query_column("frame", Some(IMAGE_EXTENSION_NAME));
        let plain = query_column("frame", None);
        assert_eq!(
            image.image_src(Some(&value)),
            Some("data:image/jpeg;base64,preview")
        );
        assert_eq!(plain.image_src(Some(&value)), None);
        assert_eq!(
            image.image_src(Some(&serde_json::json!("not-an-image"))),
            None
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn image_semantics_render_as_inline_image_columns() {
        let image = query_column("top_preview", Some(IMAGE_EXTENSION_NAME));
        let tensor = query_column("wrist_preview", Some(VARIABLE_TENSOR_EXTENSION_NAME));
        let plain = query_column("episode_id", None);

        assert!(image.is_image());
        assert!(tensor.is_image());
        assert!(!plain.is_image());
        assert_eq!(image.presentation_kind(), "Image");
        assert_eq!(tensor.presentation_kind(), "Image");
        assert_eq!(plain.presentation_kind(), "Utf8");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn nulls_and_numbers_have_stable_table_treatment() {
        assert_eq!(None.presentation_class(), "query-value-null");
        assert_eq!(
            Some(&serde_json::Value::Null).presentation_class(),
            "query-value-null"
        );
        assert_eq!(
            Some(&serde_json::json!(0.82)).presentation_class(),
            "query-value-number"
        );
        assert_eq!(
            Some(&serde_json::json!(true)).presentation_class(),
            "query-value-boolean"
        );
    }
}
