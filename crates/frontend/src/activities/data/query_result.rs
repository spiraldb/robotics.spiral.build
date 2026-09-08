//! Query execution snapshots and the shared result viewer.

use crate::client::http::EpisodeApiPath;
use crate::components::interaction::{
    Action, ActionAuxiliaryHandlers, ActionPurpose, ActionVariant, BrowserFocus, CloseIcon,
};
use dioxus::prelude::*;
use spiral_robotics_common::relations::{QueryColumn, SpiralQuerySnapshot};

pub(crate) const DEFAULT_QUERY_ROWS: usize = 100;
const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const VARIABLE_TENSOR_EXTENSION_NAME: &str = "arrow.variable_shape_tensor";
const ENCODED_MEDIA_EXTENSION_NAME: &str = "spql.media.EncodedMedia";

/// A complete, renderable snapshot of the Query tool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryExecutionPresentation {
    sql: String,
    max_rows: usize,
    state: QueryState,
}

impl QueryExecutionPresentation {
    pub(crate) fn new(sql: String, max_rows: usize, state: QueryState) -> Self {
        Self {
            sql,
            max_rows,
            state,
        }
    }

    pub(crate) fn sql(&self) -> &str {
        &self.sql
    }

    pub(crate) const fn max_rows(&self) -> usize {
        self.max_rows
    }

    pub(crate) const fn state(&self) -> &QueryState {
        &self.state
    }

    pub(crate) fn render(&self) -> Element {
        rsx! { QueryResultContent { model: self.clone() } }
    }
}

/// One immutable query execution. Editing the SQL draft or running another
/// statement never mutates an earlier tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryTab {
    id: u64,
    title: String,
    model: QueryExecutionPresentation,
}

impl QueryTab {
    pub(crate) fn new(id: u64, title: String, model: QueryExecutionPresentation) -> Self {
        Self { id, title, model }
    }

    pub(crate) const fn id(&self) -> u64 {
        self.id
    }

    pub(crate) const fn presentation(&self) -> &QueryExecutionPresentation {
        &self.model
    }

    pub(crate) fn replace_presentation(&mut self, model: QueryExecutionPresentation) {
        self.model = model;
    }

    pub(crate) fn mark_cancelled(&mut self) {
        self.model.state = QueryState::Failed("Cancelled by operator.".to_owned());
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
pub(crate) fn QueryResultView(controller: super::state::QueryController) -> Element {
    let tabs = controller.query_tabs();
    let active_id = controller.active_query();
    let active = active_id
        .and_then(|id| tabs.iter().find(|tab| tab.id == id))
        .or_else(|| tabs.last())
        .cloned();
    let tab_ids = tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();

    rsx! {
        section { class: "flex h-full min-h-0 flex-col bg-surface", "aria-label": "Query results",
            if !tabs.is_empty() {
                div { class: "query-result-tabs", role: "tablist", "aria-label": "Query results",
                    for tab in tabs.iter() {
                        div {
                            class: if active_id == Some(tab.id) { "query-result-tab-item query-result-tab-item-active ui-hover-action-host" } else { "query-result-tab-item ui-hover-action-host" },
                            Action {
                                id: "query-tab-{tab.id}",
                                purpose: ActionPurpose::Select,
                                variant: ActionVariant::Tab,
                                class: if active_id == Some(tab.id) { "query-result-tab query-result-tab-active" } else { "query-result-tab" },
                                role: "tab",
                                tabindex: if active_id == Some(tab.id) { "0" } else { "-1" },
                                aria_selected: active_id == Some(tab.id),
                                aria_controls: "query-panel-{tab.id}",
                                onclick: {
                                    let id = tab.id;
                                    move |_| controller.select_query(id)
                                },
                                auxiliary_handlers: ActionAuxiliaryHandlers::keydown({
                                    let ids = tab_ids.clone();
                                    let id = tab.id;
                                    move |event: KeyboardEvent| {
                                        let target = match event.key() {
                                            Key::ArrowLeft => ids.neighbor(id, false),
                                            Key::ArrowRight => ids.neighbor(id, true),
                                            Key::Home => ids.first().copied(),
                                            Key::End => ids.last().copied(),
                                            Key::Delete => {
                                                let target = ids.neighbor_after_close(id);
                                                controller.close_query(id);
                                                target
                                            }
                                            _ => return,
                                        };
                                        event.prevent_default();
                                        if let Some(target) = target {
                                            controller.select_query(target);
                                            target.focus_after_render();
                                        }
                                    }
                                }),
                                span { class: tab.presentation().state().tab_state_class(), "aria-hidden": "true" }
                                span { class: "query-result-tab-label", "{tab.title}" }
                            }
                            Action {
                                purpose: ActionPurpose::Close,
                                variant: ActionVariant::Icon,
                                class: "query-result-tab-close ui-hover-action",
                                aria_label: "Close {tab.title}",
                                title: "Close {tab.title}",
                                onclick: {
                                    let ids = tab_ids.clone();
                                    let id = tab.id;
                                    move |_| {
                                        let target = ids.neighbor_after_close(id);
                                        controller.close_query(id);
                                        if let Some(target) = target {
                                            controller.select_query(target);
                                            target.focus_after_render();
                                        }
                                    }
                                },
                                CloseIcon {}
                            }
                        }
                    }
                }
            }
            if let Some(tab) = active {
                div {
                    id: "query-panel-{tab.id}",
                    class: "query-result-panel",
                    role: "tabpanel",
                    tabindex: "0",
                    "aria-labelledby": "query-tab-{tab.id}",
                    header { class: "query-result-meta",
                        if let QueryState::Ready(result) = tab.presentation().state() {
                            span { class: "query-result-count", role: "status",
                                strong { "{result.rows().len()} rows" }
                                span { "{result.elapsed_ms()} ms" }
                            }
                            if result.truncated() { span { class: "chip chip-caution", "Truncated" } }
                        }
                        span { class: "query-result-limit", "Limit · {tab.presentation().max_rows()} rows" }
                        details {
                            summary { "SQL" }
                            code { "{tab.presentation().sql()}" }
                        }
                    }
                    {tab.presentation().render()}
                }
            } else {
                div { class: "flex flex-1 items-center justify-center p-6 text-center",
                    div {
                        strong { class: "block text-[12px] font-medium text-ink", "No query results" }
                        p { class: "mt-1 max-w-[52ch] text-[11px] text-ink-faint",
                            "Run a read-only query to inspect data."
                        }
                    }
                }
            }
        }
    }
}

trait QueryTabNavigation {
    fn neighbor(&self, current: u64, forward: bool) -> Option<u64>;
    fn neighbor_after_close(&self, current: u64) -> Option<u64>;
}

impl QueryTabNavigation for [u64] {
    fn neighbor(&self, current: u64, forward: bool) -> Option<u64> {
        let index = self.iter().position(|id| *id == current)?;
        let offset = if forward {
            (index + 1) % self.len()
        } else {
            (index + self.len() - 1) % self.len()
        };
        self.get(offset).copied()
    }

    fn neighbor_after_close(&self, current: u64) -> Option<u64> {
        let index = self.iter().position(|id| *id == current)?;
        index
            .checked_sub(1)
            .and_then(|index| self.get(index))
            .or_else(|| self.get(index + 1))
            .copied()
    }
}

trait QueryTabPresentation {
    fn focus_after_render(self);
}

impl QueryTabPresentation for u64 {
    fn focus_after_render(self) {
        BrowserFocus::after_render(format!("query-tab-{self}"));
    }
}

#[component]
fn QueryResultContent(model: QueryExecutionPresentation) -> Element {
    rsx! {
        match model.state() {
            QueryState::Running => rsx! {
                div { class: "flex flex-1 items-center justify-center p-6 text-[11px] text-ink-soft", role: "status",
                    span { class: "mr-2 h-1.5 w-1.5 rounded-full bg-accent" }
                    "Running query…"
                }
            },
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
            QueryState::Ready(result) => {
                let has_video_previews = result.needs_video_preview_column();
                let result_row_count = result.rows().len();
                rsx! {
                    div { class: "query-result-ready",
                        div { class: "query-result-table-scroll",
                            table { class: "query-table", "aria-label": "Query results",
                                thead { tr {
                                    if has_video_previews {
                                        th { class: "query-media-heading", scope: "col",
                                            span { class: "query-column-name", "Preview" }
                                            span { class: "query-column-type", "Video" }
                                        }
                                    }
                                    for column in &result.columns() {
                                        th { key: "{column.name()}", scope: "col",
                                            title: column.presentation_title(),
                                            span { class: "query-column-name", "{column.name()}" }
                                            span { class: "query-column-type", "{column.presentation_kind()}" }
                                        }
                                    }
                                } }
                                tbody {
                                    for (index, row) in result.rows().iter().enumerate() {
                                        tr { key: "{index}",
                                            if has_video_previews {
                                                {row.render_video_cell(index)}
                                            }
                                            for column in &result.columns() {
                                                {QueryCellContext::new(column, row, QueryRowPosition::new(index, result_row_count)).render()}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

impl QueryState {
    fn tab_state_class(&self) -> &'static str {
        match self {
            Self::Running => "query-tab-state query-tab-running",
            Self::Ready(_) => "query-tab-state query-tab-ready",
            Self::Failed(_) => "query-tab-state query-tab-failed",
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
        self.is_image()
            .then(|| value?.as_str())?
            .filter(|value| value.starts_with("data:image/"))
    }
}

struct QueryCellContext<'a> {
    column: &'a QueryColumn,
    row: &'a serde_json::Map<String, serde_json::Value>,
    row_index: usize,
    row_count: usize,
}

#[derive(Clone, Copy)]
struct QueryRowPosition {
    index: usize,
    count: usize,
}

impl QueryRowPosition {
    const fn new(index: usize, count: usize) -> Self {
        Self { index, count }
    }
}

impl<'a> QueryCellContext<'a> {
    const fn new(
        column: &'a QueryColumn,
        row: &'a serde_json::Map<String, serde_json::Value>,
        position: QueryRowPosition,
    ) -> Self {
        Self {
            column,
            row,
            row_index: position.index,
            row_count: position.count,
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

    fn render(&self) -> Element {
        let column = self.column;
        let value = self.row.get(column.name());
        if column.is_image() {
            return rsx! {
                td { key: "{column.name()}", class: "query-media-cell",
                    if let Some(src) = column.image_src(value) {
                        img {
                            class: "query-image-preview",
                            src,
                            alt: self.image_alt(),
                            loading: "lazy",
                            decoding: "async",
                            draggable: "false",
                        }
                    } else {
                        span { class: "query-image-unavailable", "No image" }
                    }
                }
            };
        }
        if column.semantic_type() == Some(ENCODED_MEDIA_EXTENSION_NAME) {
            return self.row.render_video_cell(self.row_index);
        }
        let rendered = value.cell_text();
        let display = if value.is_none_or(serde_json::Value::is_null) {
            "—"
        } else {
            rendered.as_str()
        };
        rsx! {
            td {
                key: "{column.name()}",
                class: value.presentation_class(),
                title: "{rendered}",
                "{display}"
            }
        }
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

trait QueryResultPresentation {
    fn needs_video_preview_column(&self) -> bool;
}

impl QueryResultPresentation for SpiralQuerySnapshot {
    fn needs_video_preview_column(&self) -> bool {
        if self
            .columns()
            .iter()
            .any(|column| column.semantic_type() == Some(ENCODED_MEDIA_EXTENSION_NAME))
        {
            return false;
        }
        let has = |name: &str| self.columns().iter().any(|column| column.name() == name);
        has("episode_id")
            && has("stream_id")
            && has("segment_index")
            && (has("camera_id")
                || has("byte_size")
                || self
                    .columns()
                    .iter()
                    .any(|column| column.semantic_type() == Some(ENCODED_MEDIA_EXTENSION_NAME)))
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

trait QueryRowPresentation {
    fn video_preview(&self) -> Option<(String, String)>;
    fn render_video_cell(&self, row_index: usize) -> Element;
}

impl QueryRowPresentation for serde_json::Map<String, serde_json::Value> {
    fn render_video_cell(&self, row_index: usize) -> Element {
        let Some((url, label)) = self.video_preview() else {
            return rsx! {
                td { class: "query-media-cell query-media-unavailable", title: "This row does not identify one exact video segment", "—" }
            };
        };
        rsx! {
            td { class: "query-media-cell",
                video {
                    class: "query-video-preview",
                    src: crate::client::project::ProjectClient::current().scoped_url(&url),
                    preload: "none",
                    controls: true,
                    playsinline: true,
                    "aria-label": "Query video {label}, row {row_index + 1}",
                    "Your browser cannot preview this MP4 segment."
                }
            }
        }
    }

    fn video_preview(&self) -> Option<(String, String)> {
        let episode = self.get("episode_id")?.as_str()?;
        let stream = self.get("stream_id")?.as_str()?;
        let segment = self.get("segment_index")?.as_u64()?.try_into().ok()?;
        let label = self
            .get("camera_id")
            .and_then(serde_json::Value::as_str)
            .filter(|label| !label.is_empty())
            .unwrap_or(stream)
            .to_owned();
        Some((episode.video_url(stream, segment), label))
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
    fn inline_image_alt_uses_camera_specific_identity_and_row_position() {
        let column = query_column("wrist_preview", Some(IMAGE_EXTENSION_NAME));
        let row = serde_json::Map::from_iter([
            ("episode_id".to_owned(), serde_json::json!("take-12")),
            (
                "wrist_stream_id".to_owned(),
                serde_json::json!("follower/wrist"),
            ),
            ("wrist_frame_index".to_owned(), serde_json::json!(42)),
        ]);
        assert_eq!(
            QueryCellContext::new(&column, &row, QueryRowPosition::new(2, 18)).image_alt(),
            "Wrist for episode take-12, stream follower/wrist, frame 42; result 3 of 18"
        );
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn video_rows_map_to_the_exact_range_streaming_endpoint() {
        let columns = vec![
            query_column("episode_id", None),
            query_column("stream_id", None),
            query_column("segment_index", None),
            query_column("camera_id", None),
        ];
        let row = serde_json::Map::from_iter([
            ("episode_id".to_owned(), serde_json::json!("take 1")),
            ("stream_id".to_owned(), serde_json::json!("follower/camera")),
            ("segment_index".to_owned(), serde_json::json!(2)),
            ("camera_id".to_owned(), serde_json::json!("wrist")),
        ]);
        let result = SpiralQuerySnapshot::new(columns, vec![row.clone()], false).with_elapsed_ms(1);
        assert!(result.needs_video_preview_column());
        assert_eq!(
            row.video_preview(),
            Some((
                "/api/episodes/take%201/videos/follower%2Fcamera?segment=2".to_owned(),
                "wrist".to_owned(),
            ))
        );

        let encoded = SpiralQuerySnapshot::new(
            vec![
                query_column("episode_id", None),
                query_column("stream_id", None),
                query_column("segment_index", None),
                query_column("media", Some(ENCODED_MEDIA_EXTENSION_NAME)),
            ],
            vec![row],
            false,
        )
        .with_elapsed_ms(result.elapsed_ms());
        assert!(!encoded.needs_video_preview_column());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn result_tab_keyboard_navigation_wraps_and_close_prefers_the_left_neighbor() {
        let ids = [3, 7, 9];
        assert_eq!(ids.neighbor(3, false), Some(9));
        assert_eq!(ids.neighbor(9, true), Some(3));
        assert_eq!(ids.neighbor_after_close(7), Some(3));
        assert_eq!(ids.neighbor_after_close(3), Some(7));
    }
}
