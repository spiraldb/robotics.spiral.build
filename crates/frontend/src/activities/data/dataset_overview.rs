//! One dataset at a glance: how much was recorded, and which streams exist.
use super::DatasetClient;
use crate::format::{
    ByteCountPresentation, GroupedCountPresentation, LongDurationPresentation,
    UtcTimestampPresentation, WallClockPresentation,
};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection};

/// Totals over every member, from one aggregate query.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct DatasetFacts {
    episodes: Option<u64>,
    duration_ns: Option<u64>,
    bytes: Option<u64>,
    first_start: Option<u64>,
    last_end: Option<u64>,
}

impl DatasetFacts {
    fn sql(dataset: &DatasetSelection) -> String {
        format!(
            "SELECT count(*) AS episodes, sum(duration_ns) AS duration_ns, sum(source.size) AS bytes, \
             min(message_start_time) AS first_start, max(message_end_time) AS last_end \
             FROM spql.mcap.episodes('{}')",
            dataset.sql_name().replace('\'', "''")
        )
    }
    fn chips(&self) -> Vec<String> {
        let mut chips = Vec::new();
        if let Some(episodes) = self.episodes {
            chips.push(format!("{} episodes", episodes.grouped_label()));
        }
        if let Some(duration) = self.duration_ns {
            chips.push(format!(
                "{} recorded",
                (duration as f64 / 1e9).long_duration_label()
            ));
        }
        if let Some(bytes) = self.bytes {
            chips.push(bytes.byte_label());
        }
        if let (Some(first), Some(last)) = (self.first_start, self.last_end) {
            chips.push(if !first.has_wall_clock() {
                "no wall clock".into()
            } else {
                let (first, last) = (first.utc_date_label(), last.utc_date_label());
                if first == last {
                    first
                } else {
                    format!("{first} → {last}")
                }
            });
        }
        chips
    }
}

#[component]
pub(super) fn DatasetOverview(
    dataset: DatasetSelection,
    inspection: Option<Result<DatasetInspection, String>>,
) -> Element {
    // The totals belong to this dataset alone: choosing another one re-queries
    // instead of keeping the first answer under a new name.
    let facts = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        let snapshot = DatasetClient::current()
            .run_spiral_query(DatasetFacts::sql(&dataset), 1)
            .await
            .map_err(|error| error.to_string())?;
        let row = snapshot
            .into_rows()
            .into_iter()
            .next()
            .ok_or("Dataset totals returned no row")?;
        serde_json::from_value::<DatasetFacts>(serde_json::Value::Object(row))
            .map_err(|error| format!("Reading dataset totals: {error}"))
    }));
    let label = dataset.label();
    let streams = inspection
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|detail| detail.topics().to_vec())
        .unwrap_or_default();
    rsx! {
        section { class: "data-overview", "aria-label": "Dataset overview",
            header { class: "data-overview-head",
                h2 { "{label}" }
                p { class: "data-overview-facts",
                    match &*facts.read() {
                        Some(Ok(facts)) => rsx! {
                            for chip in facts.chips() { span { class: "data-overview-chip", "{chip}" } }
                            if !streams.is_empty() { span { class: "data-overview-chip", "{streams.len()} streams" } }
                        },
                        Some(Err(error)) => rsx! { span { class: "data-overview-chip data-overview-chip-alert", role: "alert", "{error}" } },
                        None => rsx! { span { class: "data-overview-chip", role: "status", "Counting recordings…" } },
                    }
                }
            }
            section { class: "data-overview-streams", "aria-label": "Streams",
                h3 { "Streams" }
                match inspection {
                    None => rsx! { p { class: "data-rail-note", role: "status", "Reading dataset definition…" } },
                    Some(Err(error)) => rsx! { p { class: "data-rail-note", role: "alert", "{error}" } },
                    Some(Ok(detail)) if detail.topics().is_empty() => rsx! { p { class: "data-rail-note", "This dataset declares no streams yet." } },
                    Some(Ok(detail)) => rsx! {
                        table { class: "data-ledger", "aria-label": "Streams in {label}",
                            thead { tr { th { "Stream" } th { "Schema" } th { class: "text-right", "Messages" } th { class: "text-right", "Episodes" } } }
                            tbody {
                                for topic in detail.topics() {
                                    tr { key: "{topic.name()}-{topic.revision().map(|r| r.variant().to_owned()).unwrap_or_default()}",
                                        td { class: "font-medium", "{topic.name()}" }
                                        td { class: "text-ink-soft", "{topic.schemas().join(\", \")}" }
                                        td { class: "text-right tabular-nums", {topic.samples().map(GroupedCountPresentation::grouped_label).unwrap_or_else(|| "—".into())} }
                                        // A stream in every episode is the rule; the exception
                                        // is what an operator is looking for.
                                        td { class: "text-right tabular-nums",
                                            match detail.members() {
                                                Some(total) if total == topic.members() => rsx! { span { class: "text-ink-faint", title: "In every episode", "all" } },
                                                Some(total) => rsx! { span { class: "text-caution", "{topic.members()} of {total}" } },
                                                None => rsx! { "{topic.members()}" },
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn dataset_facts_read_as_short_chips() {
        let facts: DatasetFacts = serde_json::from_value(serde_json::json!({
            "episodes": 40, "duration_ns": 3_549_898_920_704_u64, "bytes": 3_509_376_768_u64,
            "first_start": 1_743_701_335_734_982_912_u64, "last_end": 1_776_208_497_899_157_760_u64
        }))
        .unwrap();
        assert_eq!(
            facts.chips(),
            [
                "40 episodes",
                "59 min 10 s recorded",
                "3.3 GiB",
                "2025-04-03 → 2026-04-14"
            ]
        );
        let sparse: DatasetFacts =
            serde_json::from_value(serde_json::json!({"episodes": 0})).unwrap();
        assert_eq!(sparse.chips(), ["0 episodes"]);
        let dataset = DatasetSelection::new(vec!["owner's data".into()]).unwrap();
        assert!(DatasetFacts::sql(&dataset).contains("'owner''s data'"));
    }
}
