//! Recording navigation within one native Spiral dataset.
use super::DatasetClient;
use crate::format::ClockPresentation;
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::native_datasets::{NativeDatasetInspection, NativeDatasetSelection};
use spiral_robotics_common::relations::SpiralQuerySnapshot;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct NativeEpisode {
    id: String,
    source_uri: Option<String>,
    message_count: Option<u64>,
    message_start_time: Option<u64>,
    message_end_time: Option<u64>,
    duration_ns: Option<u64>,
}
impl NativeEpisode {
    fn duration(&self) -> String {
        self.duration_ns
            .map(|nanos| (nanos as f64 / 1e9).clock_label())
            .unwrap_or_else(|| "Unknown duration".into())
    }
    fn time(nanos: Option<u64>) -> String {
        let Some(nanos) = nanos else {
            return "Unknown time".into();
        };
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        {
            js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((nanos / 1_000_000) as f64))
                .to_iso_string()
                .as_string()
                .map(|time| time.replace('T', " ").trim_end_matches('Z').to_owned())
                .unwrap_or_else(|| "Unknown time".into())
        }
        #[cfg(not(all(feature = "web", target_arch = "wasm32")))]
        {
            format!("{} ms since Unix epoch", nanos / 1_000_000)
        }
    }
}

struct EpisodePage {
    episodes: Vec<NativeEpisode>,
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
    fn sql(dataset: &NativeDatasetSelection, offset: usize) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, message_count, message_start_time, message_end_time, duration_ns FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source.uri LIMIT 51 OFFSET {offset}",
            dataset.sql_name().replace('\'', "''")
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct EpisodeNavigation {
    offset: Signal<usize>,
    selected: Signal<Option<(usize, NativeEpisode)>>,
}
impl EpisodeNavigation {
    pub(super) fn new() -> Self {
        Self {
            offset: Signal::new(0),
            selected: Signal::new(None),
        }
    }
    pub(super) fn reset(mut self) {
        self.offset.set(0);
        self.selected.set(None);
    }
}

#[component]
pub(super) fn NativeEpisodeBrowser(
    inspection: NativeDatasetInspection,
    navigation: EpisodeNavigation,
    on_topics: EventHandler,
) -> Element {
    let mut offset = navigation.offset;
    let mut selected = navigation.selected;
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
            header { class: "data-content-toolbar",
                button { class: "data-text-action", onclick: move |_| selected.set(None), "Back to episodes" }
            }
            div { class: "min-h-0 flex-1 overflow-auto px-5 pb-5",
                h3 { class: "mb-5 text-base font-medium", "Episode {ordinal}" }
                dl { class: "data-recording-facts",
                    dt { "Duration" } dd { "{episode.duration()}" }
                    dt { "Messages" } dd { {episode.message_count.map(|count| count.to_string()).unwrap_or_else(|| "Unavailable".into())} }
                    dt { "Start (UTC)" } dd { "{NativeEpisode::time(episode.message_start_time)}" }
                    dt { "End (UTC)" } dd { "{NativeEpisode::time(episode.message_end_time)}" }
                }
                div { class: "my-6 border-y border-line py-4 text-xs",
                    p { class: "font-medium", "Message previews for individual episodes are unavailable." }
                    button { class: "data-text-action mt-2", onclick: move |_| on_topics.call(()), "Browse dataset topics" }
                    p { class: "mt-1 text-ink-soft", "Topic previews include messages from all episodes." }
                }
                details { class: "text-xs",
                    summary { class: "cursor-pointer text-ink-soft", "Recording source" }
                    dl { class: "data-recording-facts mt-4",
                        dt { "Recording ID" } dd { class: "break-all font-mono select-text", "{episode.id}" }
                        dt { "Source" } dd { class: "break-all select-text", "{episode.source_uri.as_deref().unwrap_or(\"Source unavailable\")}" }
                    }
                }
            }
        } else {
            header { class: "data-content-toolbar",
                p { class: "text-xs text-ink-soft", "Recordings in chronological order · UTC" }
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
                                                        td { class: "whitespace-nowrap tabular-nums text-ink-soft", "{NativeEpisode::time(episode.message_start_time)}" }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
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
    #[test]
    fn malformed_episode_metadata_is_an_error_not_an_empty_catalog() {
        assert!(
            EpisodePage::from_preview(SpiralQuerySnapshot::new(
                vec![],
                vec![Default::default()],
                false
            ))
            .is_err()
        );
        let dataset = NativeDatasetSelection::new(vec!["owner's recordings".into()]).unwrap();
        assert!(EpisodePage::sql(&dataset, 50).contains("'owner''s recordings'"));
        assert!(EpisodePage::sql(&dataset, 50).ends_with("LIMIT 51 OFFSET 50"));
    }
}
