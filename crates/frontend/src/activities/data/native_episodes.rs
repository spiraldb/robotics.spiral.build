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
    fn messages(&self) -> String {
        self.message_count
            .map(|count| format!("{count} messages"))
            .unwrap_or_else(|| "Message count unavailable".into())
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

#[component]
pub(super) fn NativeEpisodeBrowser(inspection: NativeDatasetInspection) -> Element {
    let mut offset = use_signal(|| 0_usize);
    let mut selected = use_signal(|| None::<(usize, NativeEpisode)>);
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
    let pending = *page.state().read() == UseResourceState::Pending;
    let selection = selected.read().clone();
    rsx! {
        if let Some((ordinal, episode)) = selection {
            div { class: "border-b border-line p-3 space-y-2",
                button { class: "text-xs text-accent hover:underline focus-visible:outline-2 focus-visible:outline-accent", onclick: move |_| selected.set(None), "Back to episodes" }
                h3 { class: "text-sm font-medium", "Episode {ordinal}" }
                p { class: "text-xs text-ink-soft tabular-nums", "{episode.duration()} · {episode.messages()}" }
                dl { class: "space-y-1 text-[11px]",
                    dt { class: "text-ink-soft", "Start (UTC)" }
                    dd { class: "break-all tabular-nums", "{NativeEpisode::time(episode.message_start_time)}" }
                    dt { class: "pt-1 text-ink-soft", "End (UTC)" }
                    dd { class: "break-all tabular-nums", "{NativeEpisode::time(episode.message_end_time)}" }
                }
                details { class: "text-[11px]",
                    summary { class: "cursor-pointer text-ink-soft", "Recording source and ID" }
                    p { class: "mt-2 break-all font-mono select-text", "{episode.id}" }
                    p { class: "mt-2 break-all select-text", "{episode.source_uri.as_deref().unwrap_or(\"Source unavailable\")}" }
                }
            }
            p { class: "p-3 text-xs text-ink-soft", role: "status", "Message previews for this episode are currently unavailable." }
        } else {
            div { class: "flex items-center justify-between border-b border-line px-3 py-2",
                h3 { class: "text-xs font-medium", "Episodes" }
                button { class: "text-xs text-accent hover:underline focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50", disabled: pending, onclick: move |_| page.restart(), "Refresh episodes" }
            }
            if pending {
                p { class: "p-3 text-xs text-ink-soft", role: "status", "Loading episodes…" }
            } else {
                match &*page.read() {
                    Some(Err(error)) => rsx! { p { class: "p-3 text-xs text-danger", role: "alert", "{error} Use Refresh episodes to retry." } },
                    Some(Ok(page)) => rsx! {
                        if page.episodes.is_empty() { p { class: "p-3 text-xs text-ink-soft", "No episodes on this page." } }
                        ul { class: "divide-y divide-line", "aria-label": "Episodes",
                            for (index, episode) in page.episodes.iter().cloned().enumerate() {
                                {
                                    let ordinal = offset() + index + 1;
                                    rsx! {
                                        li { key: "{episode.id}",
                                            button { class: "w-full px-3 py-2 text-left hover:bg-hover focus-visible:outline-2 focus-visible:outline-accent", onclick: { let episode = episode.clone(); move |_| selected.set(Some((ordinal, episode.clone()))) },
                                                span { class: "block text-xs font-medium", "Episode {ordinal}" }
                                                span { class: "mt-1 block text-[11px] text-ink-soft tabular-nums", "{NativeEpisode::time(episode.message_start_time)}" }
                                                span { class: "mt-1 block text-[11px] text-ink-soft tabular-nums", "{episode.duration()} · {episode.messages()}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center justify-between border-t border-line p-3 text-xs",
                            button { class: "text-accent disabled:text-ink-faint disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-accent", disabled: offset() == 0, onclick: move |_| offset.set(offset().saturating_sub(50)), "Previous" }
                            span { class: "text-ink-soft tabular-nums", "Page {offset() / 50 + 1}" }
                            button { class: "text-accent disabled:text-ink-faint disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-accent", disabled: !page.has_more, onclick: move |_| offset.set(offset() + 50), "Next" }
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
        assert_eq!(page.episodes[0].messages(), "0 messages");
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
