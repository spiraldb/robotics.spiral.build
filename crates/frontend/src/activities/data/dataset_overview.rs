//! One dataset at a glance: how much was recorded, and what the checks over
//! every episode's streams found.
use super::DatasetClient;
use super::dataset_checks::{CheckedEpisode, DatasetCheckReport};
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, LongDurationPresentation,
    UtcTimestampPresentation, WallClockPresentation,
};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::sql::SqlLiteral;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Fact, FactLayout, FactList, KindMark,
};
use std::collections::HashMap;

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
             FROM spql.mcap.episodes({})",
            SqlLiteral::text(&dataset.sql_name())
        )
    }
    fn episodes_label(&self) -> String {
        self.episodes
            .map_or_else(|| "—".into(), GroupedCountPresentation::grouped_label)
    }
    fn recorded_label(&self) -> String {
        self.duration_ns.map_or_else(
            || "—".into(),
            |duration| (duration as f64 / 1e9).long_duration_label(),
        )
    }
    fn average_seconds(&self) -> Option<f64> {
        let (total, episodes) = (self.duration_ns?, self.episodes.filter(|count| *count > 0)?);
        Some(total as f64 / episodes as f64 / 1e9)
    }
    fn size_label(&self) -> String {
        self.bytes
            .map_or_else(|| "—".into(), ByteCountPresentation::byte_label)
    }
    /// The days the recordings span, when their clock is a calendar's; a
    /// simulator counting from zero has no span worth a cell.
    fn span_label(&self) -> Option<String> {
        let (first, last) = (self.first_start?, self.last_end?);
        if !first.has_wall_clock() {
            return None;
        }
        let (first, last) = (first.utc_date_label(), last.utc_date_label());
        Some(if first == last {
            first
        } else {
            format!("{first} → {last}")
        })
    }
}

/// The totals last read for each dataset, kept for the project, by SQL
/// name: a view that mounts again — the Dataset node after a step is
/// deleted, Data after Curation — paints the last answer at once while it
/// reads again, rather than a column of "Counting…".
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetOverviewMemory {
    facts: Signal<HashMap<String, DatasetFacts>>,
}

impl DatasetOverviewMemory {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            facts: Signal::new(HashMap::new()),
        })
    }
    fn current() -> Self {
        consume_context()
    }
    fn facts(self, dataset: &DatasetSelection) -> Option<DatasetFacts> {
        self.facts.read().get(&dataset.sql_name()).cloned()
    }
    /// Keep `answer` as `dataset`'s, unless it already is.
    fn remember(mut self, dataset: &DatasetSelection, answer: &DatasetFacts) {
        let key = dataset.sql_name();
        if self.facts.peek().get(&key) != Some(answer) {
            self.facts.write().insert(key, answer.clone());
        }
    }
}

/// A dataset's totals over its check report: the view Data's Overview tab
/// and Curation's Dataset node both show.
#[component]
pub(in crate::activities) fn DatasetOverview(
    dataset: DatasetSelection,
    /// Opens an episode a finding names, where the finding is.
    on_open: EventHandler<CheckedEpisode>,
) -> Element {
    // The totals belong to this dataset alone: choosing another one
    // re-queries instead of keeping the first answer under a new name.
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
    // Once read, the totals are the dataset's last answer; until they are
    // read the last answer stands in for them.
    let memory = DatasetOverviewMemory::current();
    let remembered = dataset.clone();
    use_effect(move || {
        if let Some(Ok(answer)) = &*facts.read() {
            memory.remember(&remembered, answer);
        }
    });
    let facts = facts
        .read()
        .clone()
        .or_else(|| memory.facts(&dataset).map(Ok));
    let name = dataset.display_name();
    let sql_name = dataset.sql_name();
    rsx! {
        section { class: "data-overview", "aria-label": "Dataset overview",
            div { class: "data-overview-body",
                header { class: "data-overview-head",
                    KindMark { class: "data-overview-mark", letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                    h2 { title: "{sql_name}", "{name}" }
                }
                match &facts {
                    Some(Err(error)) => rsx! { p { class: "data-overview-alert", role: "alert", "{error}" } },
                    Some(Ok(facts)) => rsx! {
                        FactList { class: "data-overview-facts", layout: FactLayout::Strip,
                            Fact { label: "Episodes", "{facts.episodes_label()}" }
                            Fact { label: "Recorded", "{facts.recorded_label()}" }
                            Fact { label: "Size", "{facts.size_label()}" }
                        }
                    },
                    None => rsx! { p { class: "data-overview-alert", role: "status", "Counting recordings…" } },
                }
                section { class: "data-overview-section", "aria-label": "Checks",
                    DatasetCheckReport { dataset: dataset.clone(), on_open }
                }
            }
        }
    }
}

/// Every dataset in the project, before one is chosen: one ledger row each,
/// with the totals that tell them apart. Import stays in the rail; an empty
/// project offers it here too, as the one thing to do.
#[component]
pub(super) fn DatasetsAtAGlance(
    datasets: Vec<DatasetSelection>,
    on_select: EventHandler<DatasetSelection>,
    on_import: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "data-glance", "aria-label": "Datasets",
            header { class: "data-glance-head",
                h2 { "Datasets" }
                p {
                    if datasets.is_empty() {
                        "A dataset is a folder of MCAP recordings; each file is one episode."
                    } else {
                        "Choose a dataset to see its episodes and streams."
                    }
                }
            }
            if datasets.is_empty() {
                Button { purpose: ActionPurpose::Import, variant: ButtonVariant::Solid, size: ButtonSize::Standard,
                    onclick: move |_| on_import.call(()), "Import MCAP recordings" }
            } else {
                DatasetLedger { datasets, on_select }
            }
        }
    }
}

/// The project's datasets as one ledger, a row each with the totals that tell
/// them apart, and a row pressed to choose it: how every surface that asks
/// for a dataset offers them — Data's home, and a curation that has none yet.
#[component]
pub(crate) fn DatasetLedger(
    datasets: Vec<DatasetSelection>,
    on_select: EventHandler<DatasetSelection>,
) -> Element {
    rsx! {
        div { class: "data-ledger-card",
            table { class: "data-ledger data-glance-ledger", "aria-label": "Datasets",
                thead { tr {
                    th { "Dataset" }
                    th { class: "is-number", "Episodes" }
                    th { class: "is-number", "Recorded" }
                    th { class: "is-number", "Average" }
                    th { class: "is-number", "Size" }
                    th { "Span" }
                } }
                tbody {
                    for (index, dataset) in datasets.into_iter().enumerate() {
                        DatasetRow { key: "{dataset.sql_name()}", dataset, order: index, on_select }
                    }
                }
            }
        }
    }
}

#[component]
fn DatasetRow(
    dataset: DatasetSelection,
    order: usize,
    on_select: EventHandler<DatasetSelection>,
) -> Element {
    let facts = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        let row = DatasetClient::current()
            .run_spiral_query(DatasetFacts::sql(&dataset), 1)
            .await
            .ok()?
            .into_rows()
            .into_iter()
            .next()?;
        serde_json::from_value::<DatasetFacts>(serde_json::Value::Object(row)).ok()
    }));
    let memory = DatasetOverviewMemory::current();
    let remembered = dataset.clone();
    use_effect(move || {
        if let Some(Some(answer)) = &*facts.read() {
            memory.remember(&remembered, answer);
        }
    });
    let facts = facts
        .read()
        .clone()
        .flatten()
        .or_else(|| memory.facts(&dataset));
    let cell =
        |read: fn(&DatasetFacts) -> String| facts.as_ref().map_or_else(|| "…".to_owned(), read);
    let chosen = dataset.clone();
    let open = dataset.clone();
    rsx! {
        tr { class: "data-glance-row", style: "--i: {order}", onclick: move |_| on_select.call(chosen.clone()),
            td {
                button { r#type: "button", class: "data-glance-name", title: "{dataset.sql_name()}",
                    onclick: move |event: MouseEvent| { event.stop_propagation(); on_select.call(open.clone()); },
                    KindMark { letter: 'D', hue: "var(--tone-kind-dataset)", label: "Dataset" }
                    span { "{dataset.display_name()}" }
                }
            }
            td { class: "is-number", {cell(DatasetFacts::episodes_label)} }
            td { class: "is-number", {cell(DatasetFacts::recorded_label)} }
            td { class: "is-number",
                {facts.as_ref().map_or_else(|| "…".to_owned(), |facts| facts.average_seconds().map_or_else(|| "—".into(), ClockPresentation::clock_label))}
            }
            td { class: "is-number", {cell(DatasetFacts::size_label)} }
            td { class: "data-glance-span",
                {facts.as_ref().map_or_else(|| "…".to_owned(), |facts| facts.span_label().unwrap_or_else(|| "—".into()))}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn dataset_facts_read_as_instrument_values() {
        let facts: DatasetFacts = serde_json::from_value(serde_json::json!({
            "episodes": 40, "duration_ns": 3_549_898_920_704_u64, "bytes": 3_509_376_768_u64,
            "first_start": 1_743_701_335_734_982_912_u64, "last_end": 1_776_208_497_899_157_760_u64
        }))
        .unwrap();
        assert_eq!(facts.episodes_label(), "40");
        assert_eq!(facts.recorded_label(), "59 min 10 s");
        assert_eq!(facts.size_label(), "3.3 GiB");
        assert_eq!(
            facts.span_label().as_deref(),
            Some("2025-04-03 → 2026-04-14")
        );
        assert!((facts.average_seconds().unwrap() - 88.747).abs() < 0.001);
        let sparse: DatasetFacts =
            serde_json::from_value(serde_json::json!({"episodes": 0})).unwrap();
        assert_eq!(sparse.span_label(), None);
        let simulated: DatasetFacts =
            serde_json::from_value(serde_json::json!({"first_start": 5, "last_end": 9})).unwrap();
        assert_eq!(simulated.span_label(), None);
        let dataset = DatasetSelection::new(vec!["owner's data".into()]).unwrap();
        assert!(DatasetFacts::sql(&dataset).contains("'owner''s data'"));
    }
}
