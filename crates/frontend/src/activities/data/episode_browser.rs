//! Recording navigation within one Spiral dataset.
use super::DatasetClient;
use crate::format::ClockPresentation;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection};
use spiral_robotics_common::relations::SpiralQuerySnapshot;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct DatasetEpisode {
    id: String,
    source_uri: Option<String>,
    message_count: Option<u64>,
    message_start_time: Option<u64>,
    message_end_time: Option<u64>,
    duration_ns: Option<u64>,
}
impl DatasetEpisode {
    fn duration(&self) -> String {
        self.duration_ns
            .map(|nanos| (nanos as f64 / 1e9).clock_label())
            .unwrap_or_else(|| "Unknown duration".into())
    }
    fn time(nanos: Option<u64>) -> String {
        let Some(nanos) = nanos else {
            return "Unknown time".into();
        };
        js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((nanos / 1_000_000) as f64))
            .to_iso_string()
            .as_string()
            .map(|time| time.replace('T', " ").trim_end_matches('Z').to_owned())
            .unwrap_or_else(|| "Unknown time".into())
    }
}

struct EpisodePage {
    episodes: Vec<DatasetEpisode>,
    has_more: bool,
}
impl EpisodePage {
    fn from_preview(preview: SpiralQuerySnapshot) -> Result<Self, String> {
        let has_more = preview.rows().len() > 50 || preview.truncated();
        let episodes = preview
            .into_rows()
            .into_iter()
            .take(50)
            .map(|row| {
                serde_json::from_value(serde_json::Value::Object(row))
                    .map_err(|error| format!("Reading episode metadata: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { episodes, has_more })
    }
    fn sql(dataset: &DatasetSelection, offset: usize) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, message_count, message_start_time, message_end_time, duration_ns FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source_uri LIMIT 51 OFFSET {offset}",
            dataset.sql_name().replace('\'', "''")
        )
    }
}

#[component]
pub(super) fn EpisodeBrowser(inspection: DatasetInspection) -> Element {
    let mut offset = use_signal(|| 0_usize);
    let mut selected = use_signal(|| None::<(usize, DatasetEpisode)>);
    let heading_id = use_hook(|| {
        format!(
            "dataset-heading-{}",
            dioxus::dioxus_core::current_scope_id().0
        )
    });
    let dataset = inspection.dataset().selection().clone();
    let mut page = use_resource(use_reactive((&dataset,), move |(dataset,)| {
        let offset = offset();
        async move {
            let response = DatasetClient::current()
                .run_spiral_query(EpisodePage::sql(&dataset, offset), 51)
                .await
                .map_err(|error| error.to_string())?;
            EpisodePage::from_preview(response)
        }
    }));
    let page_response = page.read();
    let pending = *page.state().read() == UseResourceState::Pending;
    let selection = selected.read().clone();
    rsx! {
        if let Some((ordinal, episode)) = selection {
            super::episode_topics::EpisodeInspector {
                key: "{episode.id}",
                episode: EpisodeContext::new(dataset.clone(), (ordinal, episode)),
                on_back: { let heading_id = heading_id.clone(); move |_| { selected.set(None); crate::components::interaction::BrowserFocus::after_render(heading_id.clone()); } },
            }
        } else {
            header { class: "data-inspector-heading",
                h2 { id: "{heading_id}", tabindex: "-1", "{dataset.label()}" }
                p { "{inspection.members().unwrap_or_default()} episodes" }
            }
            header { class: "data-content-toolbar",

                p { class: "text-xs text-ink-soft", "Episodes · chronological order · UTC" }
                button { class: "data-text-action", disabled: pending, onclick: move |_| page.restart(), "Refresh" }
            }
            if pending {
                p { class: "data-empty-copy", role: "status", "Loading episodes…" }
            } else {
                match &*page_response {
                    Some(Err(error)) => rsx! { p { class: "data-empty-copy text-danger", role: "alert", "{error} Use Refresh to retry." } },
                    Some(Ok(page)) => rsx! {
                        div { class: "min-h-0 flex-1 overflow-auto",
                            if page.episodes.is_empty() { p { class: "data-empty-copy", "No episodes on this page." } }
                            else {
                                table { class: "data-ledger", "aria-label": "Episodes",
                                    thead { tr { th { "Episode" } th { "Recorded (UTC)" } th { class: "text-right", "Duration" } th { class: "text-right", "Messages" } } }
                                    tbody {
                                        for (index, episode) in page.episodes.iter().cloned().enumerate() {
                                            {
                                                let ordinal = offset() + index + 1;
                                                rsx! {
                                                    tr { key: "{episode.id}",
                                                        td { button { class: "data-text-action whitespace-nowrap font-medium", onclick: { let episode = episode.clone(); move |_| selected.set(Some((ordinal, episode.clone()))) }, "Episode {ordinal}" } }
                                                        td { class: "whitespace-nowrap tabular-nums text-ink-soft", "{DatasetEpisode::time(episode.message_start_time)}" }
                                                        td { class: "text-right tabular-nums", "{episode.duration()}" }
                                                        td { class: "text-right tabular-nums", {episode.message_count.map(|count| count.to_string()).unwrap_or_else(|| "—".into())} }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        footer { class: "data-pagination",
                            span { class: "text-ink-soft tabular-nums", "{offset() + usize::from(!page.episodes.is_empty())}–{offset() + page.episodes.len()} of {inspection.members().unwrap_or_default()} episodes" }
                            div { class: "flex items-center gap-4",
                                button { class: "data-text-action", disabled: offset() == 0, onclick: move |_| offset.set(offset().saturating_sub(50)), "Previous" }
                                button { class: "data-text-action", disabled: !page.has_more, onclick: move |_| offset.set(offset() + 50), "Next" }
                            }
                        }
                    },
                    None => rsx! {},
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct EpisodeContext {
    dataset: DatasetSelection,
    ordinal: usize,
    recording: DatasetEpisode,
}
impl EpisodeContext {
    fn new(dataset: DatasetSelection, selected: (usize, DatasetEpisode)) -> Self {
        Self {
            dataset,
            ordinal: selected.0,
            recording: selected.1,
        }
    }
    pub(super) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(super) fn title(&self) -> String {
        format!("Episode {}", self.ordinal)
    }
    pub(super) fn selection(&self) -> spiral_robotics_common::datasets::EpisodeSelection {
        spiral_robotics_common::datasets::EpisodeSelection::new(
            self.dataset.clone(),
            self.recording.id.clone(),
        )
    }
    pub(super) fn timeline(&self) -> Option<super::episode_playback::EpisodeTimeRange> {
        Some(super::episode_playback::EpisodeTimeRange::new(
            self.recording.message_start_time?,
            self.recording.message_end_time?,
        ))
    }
    pub(super) fn render_summary(&self) -> Element {
        rsx! { dl { class: "data-episode-summary",
            div { dt { "Duration" } dd { "{self.recording.duration()}" } }
            div { dt { "Messages" } dd { {self.recording.message_count.map(|count| count.to_string()).unwrap_or_else(|| "Unavailable".into())} } }
            div { dt { "Recorded (UTC)" } dd { "{DatasetEpisode::time(self.recording.message_start_time)}" } }
        } }
    }
    pub(super) fn render_details(&self) -> Element {
        rsx! { details { class: "data-episode-details",
            summary { "Recording details" }
            dl { class: "data-recording-facts",
                dt { "Recording ID" } dd { "{self.recording.id}" }
                dt { "Source" } dd { {self.recording.source_uri.as_deref().unwrap_or("Source unavailable")} }
                dt { "End (UTC)" } dd { "{DatasetEpisode::time(self.recording.message_end_time)}" }
            }
        } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn episode_pages_keep_the_extra_row_for_navigation_not_display() {
        let rows = (0..51).map(|index| {
            serde_json::json!({"id": format!("recording-{index}"), "message_count": 0, "duration_ns": 0})
                .as_object().unwrap().clone()
        }).collect();
        let page =
            EpisodePage::from_preview(SpiralQuerySnapshot::new(vec![], rows, false)).unwrap();
        assert_eq!(page.episodes.len(), 50);
        assert!(page.has_more);
        assert_eq!(page.episodes[0].message_count, Some(0));
        assert_eq!(page.episodes[0].duration(), "0:00.0");
        let final_page =
            EpisodePage::from_preview(SpiralQuerySnapshot::new(vec![], vec![], false)).unwrap();
        assert!(!final_page.has_more);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn malformed_episode_metadata_is_an_error_not_an_empty_catalog() {
        assert!(
            EpisodePage::from_preview(SpiralQuerySnapshot::new(
                vec![],
                vec![Default::default()],
                false
            ))
            .is_err()
        );
        let dataset = DatasetSelection::new(vec!["owner's recordings".into()]).unwrap();
        assert!(EpisodePage::sql(&dataset, 50).contains("'owner''s recordings'"));
        assert!(EpisodePage::sql(&dataset, 50).ends_with("LIMIT 51 OFFSET 50"));
    }
}
